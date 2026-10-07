//! Whether a newer fuwa is out, for the instance's admins and for desktop apps.
//!
//! At startup and then once a day, the instance asks GitHub for fuwa's latest
//! release (a fixed address: nothing about the instance, its admins or anyone
//! on it goes with the request) and keeps what it says: the version, when it
//! came out, its notes, and its `SHA256SUMS` with the signature over them.
//! Nothing is ever installed here: admins update the image or binary
//! themselves (docs/self-hosting.md, "Updating"). `Node.versions.newer_release`
//! tells the instance's admins (only them) when there's something to update to.
//!
//! Desktop apps ask their instance rather than GitHub, so GitHub never learns
//! who uses fuwa or from where: `GET /updates/latest.json` is what the
//! instance knows, and `GET /updates/files/<name>` hands over one of the
//! latest release's desktop builds, fetched from GitHub once, checked against
//! `SHA256SUMS`, and kept in a private folder in the data folder
//! (`release-cache`) while it's the latest. There's no cap on downloads at
//! once, so nobody can hold every place; someone who stalls or reads too
//! slowly is cut off all the same. An instance can't slip in a
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

/// A reader that takes nothing for this long is cut off, and so is any hand-over past the whole limit.
const READER_STALL: Duration = Duration::from_secs(30);
const PASS_LIMIT: Duration = Duration::from_secs(30 * 60);
/// After its first minute, a hand-over slower than this on average is cut off.
const MIN_BYTES_PER_SEC: u64 = 32 * 1024;
/// How long a request waits for someone else's fetch of the same release from GitHub.
const WAIT_FOR_FETCH: Duration = Duration::from_secs(60);
/// The largest desktop build kept.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

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
    /// Where fetched desktop builds are kept, a folder per version.
    cache: std::path::PathBuf,
}

impl Releases {
    pub fn new(on: bool, cache: std::path::PathBuf) -> Arc<Self> {
        Arc::new(Self { on, latest: RwLock::new(None), cache })
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

    /// The desktop app's installers in the latest release, for the web app's
    /// download button. Each is fetched through `/updates/files/<name>`.
    pub fn desktop_app(&self) -> Option<pb::DesktopApp> {
        let latest = self.latest()?;
        let downloads: Vec<_> = latest.files.iter().filter_map(|file| download(&latest.version, file)).collect();
        (!downloads.is_empty()).then(|| pb::DesktopApp { version: latest.version.clone(), downloads })
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

    /// One of the latest release's desktop builds. Only the files listed in
    /// [`Latest::files`], so this can't be made to fetch anything else; each
    /// is fetched from GitHub once and kept while it's the latest.
    async fn pass(&self, name: &str) -> Response {
        let Some(latest) = self.latest() else {
            return plain(StatusCode::NOT_FOUND, "this instance doesn't know of a fuwa release");
        };
        let Some(file) = latest.files.iter().find(|file| file.name == name) else {
            return plain(StatusCode::NOT_FOUND, "that isn't a file of the latest fuwa release");
        };
        let Some(sha256) = sum_of(&latest.sums, &file.name) else {
            return plain(StatusCode::NOT_FOUND, "that isn't a file of the latest fuwa release");
        };
        let path = self.cache.join(&latest.version).join(&file.name);
        if !path.is_file() {
            // One fetch at a time: whoever comes next finds the copy.
            let Ok(_fetching) = tokio::time::timeout(WAIT_FOR_FETCH, FETCHING.lock()).await else {
                return plain(StatusCode::SERVICE_UNAVAILABLE, "try again in a moment");
            };
            if !path.is_file() && keep(&self.cache, &latest.version, file, sha256, &path).await.is_err() {
                tracing::warn!("couldn't fetch a fuwa desktop build from GitHub");
                crate::reports::server_error("release_pass_failed", Some("releases::pass"));
                return plain(StatusCode::BAD_GATEWAY, "GitHub didn't hand the file over; try again later");
            }
        }
        let Ok(mut source) = tokio::fs::File::open(&path).await else {
            return plain(StatusCode::SERVICE_UNAVAILABLE, "try again in a moment");
        };
        // Read on a task of its own; a reader that stalls, reads too slowly,
        // or takes too long overall ends it.
        let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<bytes::Bytes>>(4);
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            let until = started + PASS_LIMIT;
            let mut buf = vec![0u8; 64 * 1024];
            let mut sent = 0u64;
            loop {
                let secs = started.elapsed().as_secs();
                if secs >= 60 && sent / secs < MIN_BYTES_PER_SEC {
                    return;
                }
                let chunk = match tokio::io::AsyncReadExt::read(&mut source, &mut buf).await {
                    Ok(0) => return,
                    Ok(n) => Ok(bytes::Bytes::copy_from_slice(&buf[..n])),
                    Err(_) => Err(std::io::Error::other("cut off")),
                };
                let stop = chunk.is_err();
                sent += chunk.as_ref().map_or(0, |c| c.len() as u64);
                let wait = READER_STALL.min(until.saturating_duration_since(tokio::time::Instant::now()));
                if tx.send_timeout(chunk, wait).await.is_err() || stop {
                    return;
                }
            }
        });
        let body = futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|chunk| (chunk, rx)) });
        let mut response = Response::new(Body::from_stream(body));
        let h = response.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(file.size));
        // A version's file never changes.
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400, immutable"));
        response
    }
}

/// One fetch from GitHub at a time.
static FETCHING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The SHA-256 `SHA256SUMS` gives a file.
fn sum_of(sums: &str, name: &str) -> Option<[u8; 32]> {
    let line = sums.lines().find(|line| line.split_whitespace().nth(1) == Some(name))?;
    let hex = line.split_whitespace().next()?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Fetches a desktop build into `path`, whole and matching its sum, and
/// clears out older versions' copies.
async fn keep(
    cache: &std::path::Path,
    version: &str,
    file: &File,
    sha256: [u8; 32],
    path: &std::path::Path,
) -> Result<(), ()> {
    use sha2::Digest as _;
    use tokio::io::AsyncWriteExt as _;
    if file.size > MAX_FILE_BYTES {
        return Err(());
    }
    let dir = path.parent().ok_or(())?;
    private_dir(cache).await?;
    if let Ok(mut old) = tokio::fs::read_dir(cache).await {
        while let Ok(Some(entry)) = old.next_entry().await {
            if entry.file_name() != version {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
    private_dir(dir).await?;
    let url = format!("{DOWNLOAD_URL}v{version}/{}", file.name);
    let response = CLIENT.get(url).timeout(PASS_LIMIT).send().await.map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    let part = path.with_extension("part");
    let result = async {
        // Never through something already there (a link planted in its place, say).
        let _ = tokio::fs::remove_file(&part).await;
        let mut out = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&part).await.map_err(|_| ())?;
        let mut hash = sha2::Sha256::new();
        let mut done = 0u64;
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| ())?;
            done += chunk.len() as u64;
            if done > file.size {
                return Err(());
            }
            hash.update(&chunk);
            out.write_all(&chunk).await.map_err(|_| ())?;
        }
        out.sync_all().await.map_err(|_| ())?;
        if done != file.size || <[u8; 32]>::from(hash.finalize()) != sha256 {
            return Err(());
        }
        tokio::fs::rename(&part, path).await.map_err(|_| ())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

/// Makes `dir` a folder only fuwa can read (0700), refusing one that's a link.
async fn private_dir(dir: &std::path::Path) -> Result<(), ()> {
    match tokio::fs::symlink_metadata(dir).await {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(()),
        Err(_) => tokio::fs::create_dir(dir).await.map_err(|_| ())?,
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await.map_err(|_| ())?;
    }
    Ok(())
}

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

/// A desktop installer, from its name (`fuwa-desktop-<version>-<arch>-<system><ending>`,
/// as `.github/workflows/desktop.yml` names them). The bare programs aren't
/// offered: the installers put the app where the system expects it.
fn download(version: &str, file: &File) -> Option<pb::DesktopDownload> {
    use pb::{DesktopPackage as Package, DesktopSystem as System};
    let rest = file.name.strip_prefix(&format!("fuwa-desktop-{version}-"))?;
    let (arch, rest) = rest.split_once('-')?;
    let (system, package) = match rest {
        "windows-setup.exe" => (System::Windows, Package::Setup),
        "macos.dmg" => (System::Macos, Package::Dmg),
        "linux.AppImage" => (System::Linux, Package::Appimage),
        "linux.deb" => (System::Linux, Package::Deb),
        _ => return None,
    };
    Some(pb::DesktopDownload {
        path: format!("/updates/files/{}", file.name),
        size: file.size,
        system: system.into(),
        package: package.into(),
        arch: arch.into(),
    })
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
    fn only_a_newer_release_counts() {
        let releases = Releases::new(true, std::env::temp_dir());
        assert!(releases.newer().is_none());
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
        assert!(releases.newer().is_none());
        latest.version = "999.0.0".into();
        releases.set(latest);
        assert_eq!(releases.newer().unwrap().version, "999.0.0");
    }

    #[test]
    fn installers_are_offered_by_system() {
        let releases = Releases::new(true, std::env::temp_dir());
        assert!(releases.desktop_app().is_none());
        let file = |name: &str| File { name: name.into(), size: 7 };
        releases.set(Latest {
            version: "0.4.2".into(),
            published_at: String::new(),
            notes: String::new(),
            page: String::new(),
            sums: String::new(),
            signature: String::new(),
            files: vec![
                file("fuwa-desktop-0.4.2-x86_64-linux"),
                file("fuwa-desktop-0.4.2-x86_64-linux.deb"),
                file("fuwa-desktop-0.4.2-x86_64-linux.AppImage"),
                file("fuwa-desktop-0.4.2-aarch64-macos.dmg"),
                file("fuwa-desktop-0.4.2-x86_64-windows.exe"),
                file("fuwa-desktop-0.4.2-x86_64-windows-setup.exe"),
            ],
        });
        let app = releases.desktop_app().unwrap();
        assert_eq!(app.version, "0.4.2");
        let found: Vec<_> = app.downloads.iter().map(|d| (d.system(), d.package(), d.arch.as_str())).collect();
        use pb::{DesktopPackage as P, DesktopSystem as S};
        assert_eq!(
            found,
            [
                (S::Linux, P::Deb, "x86_64"),
                (S::Linux, P::Appimage, "x86_64"),
                (S::Macos, P::Dmg, "aarch64"),
                (S::Windows, P::Setup, "x86_64"),
            ]
        );
        assert_eq!(app.downloads[3].path, "/updates/files/fuwa-desktop-0.4.2-x86_64-windows-setup.exe");
        assert_eq!(app.downloads[3].size, 7);
    }

    #[test]
    fn sums_name_each_files_hash() {
        let sums = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  fuwa-desktop-1.0.0-x86_64-linux\nzz  bad\n";
        assert_eq!(sum_of(sums, "fuwa-desktop-1.0.0-x86_64-linux").unwrap()[..2], [0xba, 0x78]);
        assert!(sum_of(sums, "bad").is_none());
        assert!(sum_of(sums, "missing").is_none());
    }
}
