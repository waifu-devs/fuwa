//! Whether a newer fuwa is out, for the instance's admins and for desktop apps.
//!
//! At startup and then once a day, the instance asks GitHub for fuwa's latest
//! release (a fixed address: nothing about the instance, its admins or anyone
//! on it goes with the request) and keeps what it says: the version, when it
//! came out, its notes, and its `SHA256SUMS` with the signature over them.
//! Nothing is ever installed here: admins update the image or binary
//! themselves (docs/self-hosting.md, "Updating"). `/healthz` and
//! `Node.versions.newer_release` say when there's something to update to.
//!
//! Desktop apps ask their instance rather than GitHub, so GitHub never learns
//! who uses fuwa or from where: `GET /updates/latest.json` is what the
//! instance knows, and `GET /updates/files/<name>` passes one of the latest
//! release's desktop builds through from GitHub. An instance can't slip in a
//! build of its own: apps check the signature on `SHA256SUMS` against the
//! release key they were built with, and the file against `SHA256SUMS`,
//! before anything runs (`desktop/src/core/updates.rs`).
//!
//! `FUWA_UPDATE_CHECK=off` turns the check off; the instance then knows of no
//! release and desktop apps on it look for updates elsewhere.

use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Path as UrlPath;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::StreamExt;
use http::{HeaderValue, StatusCode, header};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::pb;

/// Where fuwa's releases are listed.
const LATEST_URL: &str = "https://api.github.com/repos/waifu-devs/fuwa/releases/latest";

/// Where a release's files are downloaded from, followed by `v<version>/<name>`.
const DOWNLOAD_URL: &str = "https://github.com/waifu-devs/fuwa/releases/download/";

/// Where a release's page is, followed by `v<version>`.
const PAGE_URL: &str = "https://github.com/waifu-devs/fuwa/releases/tag/";

/// The only hosts a check or a download ever reaches, redirects included:
/// GitHub's API, its release pages, and where it keeps release files.
const HOSTS: &[&str] =
    &["api.github.com", "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

/// The first check waits a little, so an instance that keeps restarting
/// doesn't ask GitHub each time.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// After a check that failed.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

/// The most read of GitHub's answer about a release, of `SHA256SUMS` and of its signature.
const MAX_RELEASE_BYTES: usize = 1024 * 1024;
const MAX_SUMS_BYTES: usize = 64 * 1024;
const MAX_SIGNATURE_BYTES: usize = 1024;
/// Release notes past this are cut.
const MAX_NOTES_BYTES: usize = 20_000;

/// Desktop builds passed through at once; more wait their turn.
const MAX_PASSES: usize = 16;

/// What the instance knows about fuwa's latest release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Latest {
    /// Such as "0.4.2".
    pub version: String,
    /// When it came out, as GitHub writes it (RFC 3339).
    pub published_at: String,
    /// The release's notes, in Markdown.
    pub notes: String,
    /// The release's page.
    pub page: String,
    /// The release's `SHA256SUMS`: a line per file, its SHA-256 then its name.
    pub sums: String,
    /// The release key's Ed25519 signature over `sums`, in base64. Empty for
    /// a release made before releases were signed.
    pub signature: String,
    /// The desktop builds this instance passes through, each listed in `sums`.
    pub files: Vec<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub size: u64,
}

/// The release check and what it last found.
pub struct Releases {
    on: bool,
    latest: RwLock<Option<Arc<Latest>>>,
}

impl Releases {
    pub fn new(on: bool) -> Arc<Self> {
        Arc::new(Self { on, latest: RwLock::new(None) })
    }

    /// Checks at startup (after [`FIRST_CHECK`]) and then daily, until `shutdown`.
    pub fn spawn(self: &Arc<Self>, shutdown: CancellationToken) {
        if !self.on {
            return;
        }
        let releases = self.clone();
        tokio::spawn(async move {
            let mut wait = FIRST_CHECK;
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(wait) => {}
                }
                wait = match check().await {
                    Ok(latest) => {
                        if newer(&latest.version, crate::VERSION) {
                            tracing::info!(latest = %latest.version, running = crate::VERSION, "a newer fuwa is out");
                        }
                        releases.set(latest);
                        CHECK_EVERY
                    }
                    Err(place) => {
                        // Never the cause: it could name a proxy or an internal host.
                        tracing::warn!("couldn't check for a new fuwa release; trying again in an hour");
                        crate::reports::server_error("release_check_failed", Some(place));
                        RETRY_AFTER
                    }
                };
            }
        });
    }

    /// Replaces what the instance knows (the check, and tests).
    pub fn set(&self, latest: Latest) {
        *self.latest.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(latest));
    }

    pub fn latest(&self) -> Option<Arc<Latest>> {
        self.latest.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The latest release, when it's newer than this instance.
    pub fn newer(&self) -> Option<pb::Release> {
        let latest = self.latest().filter(|latest| newer(&latest.version, crate::VERSION))?;
        Some(pb::Release {
            version: latest.version.clone(),
            published_at: published_at(&latest.published_at),
            url: latest.page.clone(),
        })
    }

    /// What `/healthz` says: "ok", and a second line when there's a newer fuwa.
    pub fn health(&self) -> String {
        match self.newer() {
            Some(release) => format!(
                "ok\nfuwa {} is out; this is {} (see Updating in docs/self-hosting.md)\n",
                release.version,
                crate::VERSION
            ),
            None => "ok".into(),
        }
    }

    /// `/updates/latest.json` and `/updates/files/<name>`, for desktop apps.
    pub fn routes(self: &Arc<Self>) -> Router {
        let (manifest, files) = (self.clone(), self.clone());
        Router::new()
            .route(
                "/updates/latest.json",
                get(move || {
                    let releases = manifest.clone();
                    async move { releases.manifest() }
                }),
            )
            .route(
                "/updates/files/{name}",
                get(move |UrlPath(name): UrlPath<String>| {
                    let releases = files.clone();
                    async move { releases.pass(&name).await }
                }),
            )
    }

    fn manifest(&self) -> Response {
        let Some(latest) = self.latest() else {
            return plain(StatusCode::NOT_FOUND, "this instance doesn't know of a fuwa release");
        };
        let Ok(json) = serde_json::to_vec(&*latest) else {
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "couldn't write the release down");
        };
        let mut response = Response::new(Body::from(json));
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        // Apps ask now and then; a few minutes' copy at an edge is fine.
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
        response
    }

    /// One of the latest release's desktop builds, from GitHub. Only the
    /// files listed in [`Latest::files`], so this can't be made to fetch
    /// anything else.
    async fn pass(&self, name: &str) -> Response {
        let Some(latest) = self.latest() else {
            return plain(StatusCode::NOT_FOUND, "this instance doesn't know of a fuwa release");
        };
        let Some(file) = latest.files.iter().find(|file| file.name == name) else {
            return plain(StatusCode::NOT_FOUND, "that isn't a file of the latest fuwa release");
        };
        let Ok(permit) = PASSES.acquire().await else {
            return plain(StatusCode::SERVICE_UNAVAILABLE, "try again in a moment");
        };
        let url = format!("{DOWNLOAD_URL}v{}/{}", latest.version, file.name);
        let response = match CLIENT.get(url).timeout(Duration::from_secs(30 * 60)).send().await {
            Ok(response) if response.status().is_success() => response,
            _ => {
                tracing::warn!("couldn't fetch a fuwa desktop build from GitHub");
                crate::reports::server_error("release_pass_failed", Some("releases::pass"));
                return plain(StatusCode::BAD_GATEWAY, "GitHub didn't hand the file over; try again later");
            }
        };
        // The permit goes with the body, so it's held until the file's through.
        let size = file.size;
        let body = response.bytes_stream().map(move |chunk| {
            let _held = &permit;
            chunk.map_err(|_| std::io::Error::other("cut off"))
        });
        let mut response = Response::new(Body::from_stream(body));
        let h = response.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
        // A version's file never changes.
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400, immutable"));
        response
    }
}

static PASSES: Semaphore = Semaphore::const_new(MAX_PASSES);

/// Reaches only [`HOSTS`], over https, and sends nothing but a fixed user
/// agent (GitHub's API wants one).
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("fuwa-release-check")
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if !allowed(attempt.url()) {
                attempt.error("a redirect off GitHub")
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(60))
        .build()
        .expect("the release check's settings are valid")
});

fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && url.host_str().is_some_and(|host| HOSTS.iter().any(|allowed| host.eq_ignore_ascii_case(allowed)))
}

/// What GitHub says about a release (only what's read).
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
}

/// Asks GitHub for the latest release. The error names where it failed, for the anonymous report.
async fn check() -> Result<Latest, &'static str> {
    let body = fetch_bytes(LATEST_URL, MAX_RELEASE_BYTES, Some("application/vnd.github+json"))
        .await
        .map_err(|_| "releases::latest")?;
    let release: GitHubRelease = serde_json::from_slice(&body).map_err(|_| "releases::read")?;
    let version = version_of(&release).ok_or("releases::version")?;
    let names: Vec<&str> = release.assets.iter().map(|asset| asset.name.as_str()).collect();
    let file = |name: &str| format!("{DOWNLOAD_URL}v{version}/{name}");
    let sums = match names.contains(&"SHA256SUMS") {
        true => fetch_bytes(&file("SHA256SUMS"), MAX_SUMS_BYTES, None).await.map_err(|_| "releases::sums")?,
        false => Vec::new(),
    };
    let signature = match names.contains(&"SHA256SUMS.sig") {
        true => {
            fetch_bytes(&file("SHA256SUMS.sig"), MAX_SIGNATURE_BYTES, None).await.map_err(|_| "releases::signature")?
        }
        false => Vec::new(),
    };
    let sums = String::from_utf8(sums).map_err(|_| "releases::sums")?;
    let signature = String::from_utf8(signature).map_err(|_| "releases::signature")?.trim().to_string();
    Ok(latest(&release, version, sums, signature))
}

/// What's kept of a release GitHub described.
fn latest(release: &GitHubRelease, version: String, sums: String, signature: String) -> Latest {
    let prefix = format!("fuwa-desktop-{version}-");
    let listed = |name: &str| sums.lines().any(|line| line.split_whitespace().nth(1) == Some(name));
    let files = release
        .assets
        .iter()
        .filter(|asset| asset.name.starts_with(&prefix) && plain_name(&asset.name) && listed(&asset.name))
        .map(|asset| File { name: asset.name.clone(), size: asset.size })
        .collect();
    Latest {
        page: format!("{PAGE_URL}v{version}"),
        published_at: release.published_at.clone().unwrap_or_default(),
        notes: cut(release.body.as_deref().unwrap_or_default(), MAX_NOTES_BYTES),
        version,
        sums,
        signature,
        files,
    }
}

/// The release's version, from a tag like v0.4.2; None for drafts,
/// prereleases and tags of any other shape.
fn version_of(release: &GitHubRelease) -> Option<String> {
    let version = release.tag_name.strip_prefix('v')?;
    (!release.draft && !release.prerelease && parse(version).is_some()).then(|| version.to_string())
}

/// Letters, digits and `.-_` only, so a name is safe in a path.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

async fn fetch_bytes(url: &str, max: usize, accept: Option<&'static str>) -> Result<Vec<u8>, ()> {
    let mut request = CLIENT.get(url);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = request.send().await.map_err(|_| ())?;
    if !response.status().is_success() || response.content_length().is_some_and(|length| length > max as u64) {
        return Err(());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if body.len() + chunk.len() > max {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A plain version number, such as 0.4.2.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| {
        (!part.is_empty() && part.len() <= 6 && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok())?
    });
    let found = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(found)
}

/// Whether `latest` is a newer version than `running`.
pub fn newer(latest: &str, running: &str) -> bool {
    match (parse(latest), parse(running)) {
        (Some(latest), Some(running)) => latest > running,
        _ => false,
    }
}

fn published_at(rfc3339: &str) -> Option<prost_types::Timestamp> {
    rfc3339.parse::<prost_types::Timestamp>().ok()
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(json: &str) -> GitHubRelease {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        // Anything but a plain version is never newer.
        assert!(!newer("0.3.0-beta.1", "0.2.0"));
        assert!(!newer("0.3", "0.2.0"));
        assert!(!newer("0.3.0", "dev"));
        assert!(!newer("+1.0.0", "0.1.0"));
    }

    #[test]
    fn only_plain_release_tags_count() {
        let tag = |json: &str| version_of(&release(json));
        assert_eq!(tag(r#"{"tag_name":"v0.4.2"}"#).as_deref(), Some("0.4.2"));
        assert_eq!(tag(r#"{"tag_name":"0.4.2"}"#), None);
        assert_eq!(tag(r#"{"tag_name":"v0.4.2","prerelease":true}"#), None);
        assert_eq!(tag(r#"{"tag_name":"v0.4.2","draft":true}"#), None);
        assert_eq!(tag(r#"{"tag_name":"v0.4.2/../x"}"#), None);
    }

    #[test]
    fn only_listed_desktop_builds_are_passed_through() {
        let gh = release(
            r###"{
                "tag_name": "v0.4.2",
                "body": "## What's new\n* things",
                "published_at": "2026-10-04T01:00:00Z",
                "assets": [
                    {"name": "fuwa-0.4.2-x86_64-linux", "size": 10},
                    {"name": "fuwa-desktop-0.4.2-x86_64-linux", "size": 20},
                    {"name": "fuwa-desktop-0.4.2-x86_64-windows.exe", "size": 30},
                    {"name": "fuwa-desktop-0.4.2-aarch64-macos.dmg", "size": 40},
                    {"name": "fuwa-desktop-0.4.1-x86_64-linux", "size": 50},
                    {"name": "SHA256SUMS", "size": 1},
                    {"name": "SHA256SUMS.sig", "size": 1}
                ]
            }"###,
        );
        let sums = "aa  fuwa-0.4.2-x86_64-linux\nbb  fuwa-desktop-0.4.2-x86_64-linux\ncc  fuwa-desktop-0.4.2-x86_64-windows.exe\ndd  fuwa-desktop-0.4.1-x86_64-linux\n";
        let latest = latest(&gh, "0.4.2".into(), sums.into(), "c2ln".into());
        let names: Vec<&str> = latest.files.iter().map(|f| f.name.as_str()).collect();
        // The server's own binary, other versions and files missing from SHA256SUMS stay out.
        assert_eq!(names, ["fuwa-desktop-0.4.2-x86_64-linux", "fuwa-desktop-0.4.2-x86_64-windows.exe"]);
        assert_eq!(latest.files[0].size, 20);
        assert_eq!(latest.page, "https://github.com/waifu-devs/fuwa/releases/tag/v0.4.2");
        assert_eq!(latest.notes, "## What's new\n* things");
        assert_eq!(published_at(&latest.published_at).map(|t| t.seconds), Some(1_791_075_600));
    }

    #[test]
    fn checks_and_downloads_stay_on_github() {
        let ok = |url: &str| allowed(&reqwest::Url::parse(url).unwrap());
        assert!(ok("https://api.github.com/repos/waifu-devs/fuwa/releases/latest"));
        assert!(ok("https://github.com/waifu-devs/fuwa/releases/download/v0.4.2/SHA256SUMS"));
        assert!(ok("https://objects.githubusercontent.com/github-production-release-asset/1"));
        assert!(ok("https://release-assets.githubusercontent.com/github-production-release-asset/1"));
        assert!(!ok("http://github.com/waifu-devs/fuwa"));
        assert!(!ok("https://github.com:8443/waifu-devs/fuwa"));
        assert!(!ok("https://user@github.com/waifu-devs/fuwa"));
        assert!(!ok("https://github.com.example.com/x"));
        assert!(!ok("https://169.254.169.254/latest/meta-data"));
        assert!(!ok("https://localhost/x"));
    }

    #[test]
    fn names_are_plain() {
        assert!(plain_name("fuwa-desktop-0.4.2-x86_64-windows.exe"));
        assert!(!plain_name("../SHA256SUMS"));
        assert!(!plain_name("a/b"));
        assert!(!plain_name(".hidden"));
        assert!(!plain_name(""));
    }

    #[test]
    fn notes_are_cut_on_a_character() {
        assert_eq!(cut("héllo", 2), "h…");
        assert_eq!(cut("hello", 10), "hello");
    }

    #[test]
    fn health_names_a_newer_release_only() {
        let releases = Releases::new(true);
        assert_eq!(releases.health(), "ok");
        let mut latest = Latest {
            version: crate::VERSION.into(),
            published_at: "2026-10-04T01:00:00Z".into(),
            notes: String::new(),
            page: format!("{PAGE_URL}v{}", crate::VERSION),
            sums: String::new(),
            signature: String::new(),
            files: Vec::new(),
        };
        releases.set(latest.clone());
        assert_eq!(releases.health(), "ok");
        assert!(releases.newer().is_none());
        latest.version = "999.0.0".into();
        releases.set(latest);
        assert!(releases.health().starts_with("ok\nfuwa 999.0.0 is out; this is "));
        assert_eq!(releases.newer().unwrap().version, "999.0.0");
    }
}
