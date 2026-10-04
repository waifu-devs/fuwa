//! Keeping the app itself up to date.
//!
//! A little after starting and then every few hours, the app asks one of
//! the instances you added what fuwa's latest release is
//! (`GET /updates/latest.json`, see `server/src/releases.rs`) and, when it's
//! newer than this app, fetches the build for this computer through that
//! instance too (`GET /updates/files/<name>`). GitHub, where releases live,
//! never sees your address; the instance only sees that an app asked, as it
//! does for any picture. Nothing about you or this computer goes with either
//! request.
//!
//! The instance is only a courier: before anything is put in place, the
//! release's `SHA256SUMS` must carry a valid Ed25519 signature from one of
//! the keys in `desktop/release-keys.txt` (built into the app; the private
//! key never leaves the release workflow), name the exact file for this
//! version and computer, and the file's SHA-256 must match it. A release
//! that isn't newer than this app is never installed, so nobody can hand an
//! older, signed build back.
//!
//! Nothing is ever forced: a checked download only waits beside the program
//! (the bare program, or the AppImage). It replaces it when you press
//! "Restart to update", which checks it once more first; quit without
//! pressing it and the same version starts next time, with the download
//! still waiting for you. Where the app can't replace itself (installed by
//! a package manager, a folder it can't write, a computer there's no build
//! for, or a release that isn't signed) it says a new version is out and
//! where to get it. "Download updates in the background" off only stops it
//! fetching by itself.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use parking_lot::Mutex;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::core::Core;
use crate::core::reports;

/// The release keys this app trusts, one base64 Ed25519 public key a line.
const RELEASE_KEYS: &str = include_str!("../../release-keys.txt");

/// The first look waits until the app has settled in.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// What the instance says about a release can't be bigger than this.
const MAX_MANIFEST: usize = 2 * 1024 * 1024;
/// No build of the app is anywhere near this.
const MAX_FILE: u64 = 1024 * 1024 * 1024;

/// A release, as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// Markdown, from the release on GitHub.
    pub notes: String,
    /// The release's page on github.com.
    pub page: String,
}

/// Why a newer version has to be installed by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manual {
    /// "Download updates in the background" is off: one click fetches it.
    Off,
    /// The release isn't signed by a key this app knows (or no key is built in yet).
    Unsigned,
    /// There's no build of the app for this computer in the release.
    NoBuild,
    /// A package manager put the app here (the .deb); it updates it.
    Package,
    /// A macOS app bundle, signed as a whole: swapping the program inside it
    /// would break its signature, so the new one comes from the release page.
    Bundle,
    /// The app can't write where it's installed.
    ReadOnly,
    /// A build made from source, which updates by building again.
    Development,
}

impl Manual {
    pub fn explain(self) -> &'static str {
        match self {
            Manual::Off => {
                "Background downloads are off. Download it now; it installs only when you restart to update."
            }
            Manual::Unsigned => {
                "This release isn't signed with a key this app trusts, so it won't install it. Download it from the release page."
            }
            Manual::NoBuild => "There's no build for this computer in this release. Download it from the release page.",
            Manual::Package => {
                "Your package manager installed fuwa, so update it there, or take the new .deb from the release page."
            }
            Manual::Bundle => "Download the new fuwa.app from the release page and drag it over this one.",
            Manual::ReadOnly => {
                "fuwa can't write to the folder it's installed in. Download the new version from the release page."
            }
            Manual::Development => "This is a build made from source. Pull and build again to update.",
        }
    }
}

/// Where updating is at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    /// Not looked yet.
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer version this app won't fetch and put in place by itself.
    Available {
        release: Release,
        why: Manual,
    },
    Downloading {
        release: Release,
        done: u64,
        total: u64,
    },
    /// Downloaded and checked, waiting for "Restart to update".
    Ready {
        release: Release,
    },
    /// The last look or download didn't work out; it tries again later.
    Failed {
        what: &'static str,
    },
}

impl Status {
    pub fn release(&self) -> Option<&Release> {
        match self {
            Status::Available { release, .. } | Status::Downloading { release, .. } | Status::Ready { release } => {
                Some(release)
            }
            _ => None,
        }
    }
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);
/// One look or download at a time.
static BUSY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// The program this app started as, before an update replaced it.
static PROGRAM: OnceLock<Option<PathBuf>> = OnceLock::new();
/// The checked download waiting for "Restart to update".
static STAGED: Mutex<Option<Staged>> = Mutex::new(None);

struct Staged {
    file: PathBuf,
    target: PathBuf,
    sha256: [u8; 32],
}

pub fn status() -> Status {
    STATUS.lock().clone()
}

fn set(core: &Core, status: Status) {
    *STATUS.lock() = status;
    core.shared.update(|_| {});
}

/// What an instance says about the latest release (see `server/src/releases.rs`).
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub sums: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestFile {
    pub name: String,
    pub size: u64,
}

/// The file to fetch, as `SHA256SUMS` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub name: String,
    pub sha256: [u8; 32],
    pub size: Option<u64>,
}

/// What a manifest means for this app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    UpToDate,
    /// Signed, with a build for this computer: fetch `Plan`.
    Install(Plan),
    Manual(Manual),
    /// Signed by none of the keys, or not over these sums: someone tampered with it.
    Forged,
}

/// Which build this computer takes: its name in a release after the
/// version, as `release.yml` names them. None where there's no build.
pub fn build_name(appimage: bool) -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(if appimage { "x86_64-linux.AppImage" } else { "x86_64-linux" })
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-macos")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-windows.exe")
    } else {
        None
    }
}

/// The keys in `release-keys.txt` (comments and blank lines skipped).
pub fn release_keys(text: &str) -> Vec<[u8; 32]> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| base64::engine::general_purpose::STANDARD.decode(line).ok()?.try_into().ok())
        .collect()
}

/// Decides what to do with a manifest: nothing unless it's newer than
/// `running`, signed by one of `keys`, and names a build for `build`
/// (`build_name`) with its SHA-256.
pub fn judge(manifest: &Manifest, running: &str, keys: &[[u8; 32]], build: Option<&str>) -> Verdict {
    if !newer(&manifest.version, running) {
        return Verdict::UpToDate;
    }
    if keys.is_empty() || manifest.signature.trim().is_empty() {
        return Verdict::Manual(Manual::Unsigned);
    }
    let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(manifest.signature.trim()) else {
        return Verdict::Forged;
    };
    let signed = keys.iter().any(|key| {
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(manifest.sums.as_bytes(), &signature)
            .is_ok()
    });
    if !signed {
        return Verdict::Forged;
    }
    let Some(build) = build else { return Verdict::Manual(Manual::NoBuild) };
    // The version comes from the signed name, never from what the instance says alone.
    let name = format!("fuwa-desktop-{}-{build}", manifest.version);
    let Some(sha256) = manifest.sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name).then(|| hex32(hash)).flatten()
    }) else {
        return Verdict::Manual(Manual::NoBuild);
    };
    let size = manifest.files.iter().find(|f| f.name == name).map(|f| f.size);
    Verdict::Install(Plan { name, sha256, size })
}

fn hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
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

/// Where the new version goes, or why it can't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The file replaced: the program, or the AppImage it runs from.
    pub file: PathBuf,
    pub appimage: bool,
}

/// Remembers the program this app runs as. Called once at startup, before
/// an update can replace it (Linux then names the old one "(deleted)").
pub fn remember_program() {
    PROGRAM.get_or_init(|| {
        if let Some(appimage) = std::env::var_os("APPIMAGE").map(PathBuf::from)
            && appimage.is_absolute()
            && appimage.is_file()
        {
            return Some(appimage);
        }
        std::env::current_exe().ok().and_then(|exe| exe.canonicalize().ok())
    });
    // What a Windows update left behind, once the new program is surely there:
    // if both renames failed, the old copy is all there is.
    if let Some(Some(program)) = PROGRAM.get()
        && program.is_file()
    {
        let _ = std::fs::remove_file(old_copy(program));
    }
}

/// The program to start again after an update.
pub fn program() -> Option<PathBuf> {
    PROGRAM.get().cloned().flatten()
}

fn target() -> Result<Target, Manual> {
    if cfg!(debug_assertions) && std::env::var_os("FUWA_DESKTOP_UPDATE_ANYWAY").is_none() {
        return Err(Manual::Development);
    }
    let file = program().ok_or(Manual::ReadOnly)?;
    let appimage = std::env::var_os("APPIMAGE").is_some_and(|a| Path::new(&a) == file);
    if cfg!(target_os = "linux") && !appimage && (file.starts_with("/usr") || file.starts_with("/opt")) {
        return Err(Manual::Package);
    }
    if in_bundle(&file) {
        return Err(Manual::Bundle);
    }
    let dir = file.parent().ok_or(Manual::ReadOnly)?;
    // Can a file be made beside it?
    let probe = dir.join(format!(".fuwa-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(Target { file, appimage })
        }
        Err(_) => Err(Manual::ReadOnly),
    }
}

/// Whether the program runs from inside a macOS app bundle.
fn in_bundle(program: &Path) -> bool {
    program.ancestors().any(|dir| dir.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app")))
}

/// Whether the app may ask an instance about updates: over https, or plain
/// http only to this computer or a local network.
pub fn courier_allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<http::Uri>() else { return false };
    match uri.scheme_str() {
        Some("https") => true,
        Some("http") => uri.host().is_some_and(local_host),
        _ => false,
    }
}

fn local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".local") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// Where Windows keeps the old program while the new one takes its name.
fn old_copy(program: &Path) -> PathBuf {
    let mut name = program.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    program.with_file_name(name)
}

/// The file being downloaded, beside the one it replaces (so moving it in
/// is one rename on the same disk).
fn partial(target: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(".update");
    target.with_file_name(name)
}

/// Puts a checked download in place of `target`, all at once: the running
/// app keeps the old file open until it quits, and the next start runs the
/// new one. Windows won't replace a running program, but lets it be renamed.
pub fn put_in_place(new: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    }
    if cfg!(windows) {
        let old = old_copy(target);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(target, &old)?;
        if let Err(err) = std::fs::rename(new, target) {
            let _ = std::fs::rename(&old, target);
            return Err(err);
        }
        return Ok(());
    }
    std::fs::rename(new, target)
}

/// The SHA-256 of a file, or None when it can't be read.
fn sha256_of(path: &Path) -> Option<[u8; 32]> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).ok()?;
    Some(hash.finalize().into())
}

/// What the person chose with "Restart to update": the waiting download,
/// checked once more, takes the program's place, and the program starts
/// again, waiting for this one to let go of the app lock. The window quits
/// right after.
pub fn restart() -> std::io::Result<()> {
    let program = program().ok_or_else(|| std::io::Error::other("no program"))?;
    if let Some(staged) = STAGED.lock().take() {
        let failed = if sha256_of(&staged.file) != Some(staged.sha256) {
            let _ = std::fs::remove_file(&staged.file);
            Some(("update_hash_bad", "The download changed after it was checked, so it was thrown away."))
        } else if put_in_place(&staged.file, &staged.target).is_err() {
            Some(("update_apply_failed", "The new version couldn't be put in place; it'll try again later."))
        } else {
            None
        };
        if let Some((kind, what)) = failed {
            tracing::warn!("{what}");
            reports::error(kind, "core/updates.rs");
            *STATUS.lock() = Status::Failed { what };
            return Err(std::io::Error::other("not updated"));
        }
    }
    std::process::Command::new(program).args(std::env::args_os().skip(1)).env(AFTER_UPDATE, "1").spawn().map(|_| ())
}

/// Set on the app started by `restart`, so it waits for the old one to quit.
pub const AFTER_UPDATE: &str = "FUWA_DESKTOP_AFTER_UPDATE";

impl Core {
    /// Looks for updates a little after starting, then every few hours.
    pub(crate) fn watch_updates(self: &Arc<Self>) {
        remember_program();
        let weak = Arc::downgrade(self);
        self.handle().spawn(async move {
            tokio::time::sleep(FIRST_CHECK).await;
            loop {
                let Some(core) = weak.upgrade() else { return };
                let install = core.prefs().auto_update;
                core.check_for_update(install).await;
                drop(core);
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    /// Asks the instances you added, in order, until one knows the latest
    /// release; then fetches and checks it when `install`, for the person
    /// to put in place with "Restart to update".
    pub async fn check_for_update(self: &Arc<Self>, install: bool) {
        let Ok(_busy) = BUSY.try_lock() else { return };
        if matches!(status(), Status::Ready { .. }) {
            return;
        }
        let started = std::time::Instant::now();
        set(self, Status::Checking);
        let urls: Vec<String> = self
            .shared
            .read(|s| s.order.clone())
            .iter()
            .filter_map(|key| self.api(key).map(|api| api.url))
            .filter(|url| courier_allowed(url))
            .collect();
        if urls.is_empty() {
            // Nowhere to ask until an instance is added.
            return set(self, Status::Idle);
        }
        let mut found = None;
        for url in &urls {
            if let Ok(manifest) = fetch_manifest(url).await {
                found = Some((url.clone(), manifest));
                break;
            }
        }
        reports::timing("updates.check", started.elapsed());
        let Some((url, manifest)) = found else {
            reports::error("update_check_failed", "core/updates.rs");
            set(self, Status::Failed { what: "None of your instances could say what the latest fuwa is." });
            return;
        };
        let target = target();
        let appimage = target.as_ref().is_ok_and(|t| t.appimage);
        let release = Release {
            page: format!("https://github.com/waifu-devs/fuwa/releases/tag/v{}", manifest.version),
            version: manifest.version.clone(),
            notes: manifest.notes.clone(),
        };
        let plan = match judge(&manifest, env!("CARGO_PKG_VERSION"), &release_keys(RELEASE_KEYS), build_name(appimage))
        {
            Verdict::UpToDate => return set(self, Status::UpToDate),
            Verdict::Forged => {
                reports::error("update_signature_bad", "core/updates.rs");
                return set(
                    self,
                    Status::Failed { what: "The update's signature didn't check out, so it was left alone." },
                );
            }
            Verdict::Manual(why) => return set(self, Status::Available { release, why }),
            Verdict::Install(plan) => plan,
        };
        let target = match target {
            Ok(target) => target,
            Err(why) => return set(self, Status::Available { release, why }),
        };
        let part = partial(&target.file);
        let staged = Staged { file: part.clone(), target: target.file.clone(), sha256: plan.sha256 };
        // Fetched and checked on an earlier run, and still waiting.
        if sha256_of(&part) == Some(plan.sha256) {
            *STAGED.lock() = Some(staged);
            return set(self, Status::Ready { release });
        }
        if !install {
            return set(self, Status::Available { release, why: Manual::Off });
        }
        reports::used("updates.download");
        let total = plan.size.unwrap_or(0);
        set(self, Status::Downloading { release: release.clone(), done: 0, total });
        let started = std::time::Instant::now();
        let fetched = download(self, &url, &plan, &part, &release).await;
        reports::timing("updates.download", started.elapsed());
        let failed = match fetched {
            Ok(()) => None,
            Err(Fetch::Mismatch) => {
                Some(("update_hash_bad", "The download didn't match the signed checksum, so it was thrown away."))
            }
            Err(Fetch::Failed) => {
                Some(("update_download_failed", "The download didn't finish; it'll try again later."))
            }
        };
        match failed {
            None => {
                tracing::info!(version = %release.version, "a new version of fuwa is downloaded, waiting for Restart to update");
                *STAGED.lock() = Some(staged);
                set(self, Status::Ready { release })
            }
            Some((kind, what)) => {
                let _ = std::fs::remove_file(&part);
                tracing::warn!("{what}");
                reports::error(kind, "core/updates.rs");
                set(self, Status::Failed { what })
            }
        }
    }
}

enum Fetch {
    Failed,
    Mismatch,
}

fn client() -> hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Empty<Bytes>,
> {
    let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
        Ok(roots) => roots,
        Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
    };
    hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(roots.https_or_http().enable_http1().build())
}

/// A GET to the instance with nothing but a fixed user agent. Redirects
/// aren't followed: the instance answers these itself.
fn get(url: &str) -> anyhow::Result<http::Request<Empty<Bytes>>> {
    Ok(http::Request::get(url)
        .header(http::header::USER_AGENT, "fuwa-desktop")
        .header(http::header::ACCEPT, "application/json, application/octet-stream")
        .body(Empty::new())?)
}

async fn fetch_manifest(instance: &str) -> anyhow::Result<Manifest> {
    let url = format!("{}/updates/latest.json", instance.trim_end_matches('/'));
    let response = tokio::time::timeout(Duration::from_secs(20), client().request(get(&url)?)).await??;
    anyhow::ensure!(response.status().is_success(), "no release here");
    let body = http_body_util::Limited::new(response.into_body(), MAX_MANIFEST);
    let bytes = tokio::time::timeout(Duration::from_secs(20), body.collect())
        .await?
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .to_bytes();
    Ok(serde_json::from_slice(&bytes)?)
}

/// Fetches `plan` into `part`, checking its SHA-256 as it comes.
async fn download(core: &Core, instance: &str, plan: &Plan, part: &Path, release: &Release) -> Result<(), Fetch> {
    let url = format!("{}/updates/files/{}", instance.trim_end_matches('/'), plan.name);
    let request = get(&url).map_err(|_| Fetch::Failed)?;
    let response = tokio::time::timeout(Duration::from_secs(30), client().request(request))
        .await
        .map_err(|_| Fetch::Failed)?
        .map_err(|_| Fetch::Failed)?;
    if !response.status().is_success() {
        return Err(Fetch::Failed);
    }
    let limit = plan.size.unwrap_or(MAX_FILE).min(MAX_FILE);
    let mut file = std::fs::File::create(part).map_err(|_| Fetch::Failed)?;
    let mut hash = Sha256::new();
    let (mut done, mut shown) = (0u64, 0u64);
    let mut body = response.into_body();
    loop {
        // Nothing for a minute means the download stalled.
        let frame = match tokio::time::timeout(Duration::from_secs(60), body.frame()).await {
            Ok(Some(Ok(frame))) => frame,
            Ok(None) => break,
            _ => return Err(Fetch::Failed),
        };
        let Ok(chunk) = frame.into_data() else { continue };
        done += chunk.len() as u64;
        if done > limit {
            return Err(Fetch::Mismatch);
        }
        hash.update(&chunk);
        file.write_all(&chunk).map_err(|_| Fetch::Failed)?;
        if done - shown >= 512 * 1024 {
            shown = done;
            set(core, Status::Downloading { release: release.clone(), done, total: plan.size.unwrap_or(0) });
        }
    }
    file.sync_all().map_err(|_| Fetch::Failed)?;
    if <[u8; 32]>::from(hash.finalize()) != plan.sha256 {
        return Err(Fetch::Mismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair as _;

    fn key() -> ring::signature::Ed25519KeyPair {
        ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap()
    }

    fn public(pair: &ring::signature::Ed25519KeyPair) -> [u8; 32] {
        pair.public_key().as_ref().try_into().unwrap()
    }

    fn manifest(version: &str, sums: &str, pair: &ring::signature::Ed25519KeyPair) -> Manifest {
        Manifest {
            version: version.into(),
            notes: String::new(),
            sums: sums.into(),
            signature: base64::engine::general_purpose::STANDARD.encode(pair.sign(sums.as_bytes()).as_ref()),
            files: vec![ManifestFile { name: "fuwa-desktop-0.5.0-x86_64-linux".into(), size: 3 }],
        }
    }

    const HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn a_newer_signed_build_for_this_computer_installs() {
        let pair = key();
        let sums = format!("{HASH}  fuwa-desktop-0.5.0-x86_64-linux\n{HASH}  fuwa-0.5.0-x86_64-linux\n");
        let verdict = judge(&manifest("0.5.0", &sums, &pair), "0.4.0", &[public(&pair)], Some("x86_64-linux"));
        let Verdict::Install(plan) = verdict else { panic!("{verdict:?}") };
        assert_eq!(plan.name, "fuwa-desktop-0.5.0-x86_64-linux");
        assert_eq!(plan.sha256, hex32(HASH).unwrap());
        assert_eq!(plan.size, Some(3));
        // sha256sum's binary mode marks names with a star.
        let starred = format!("{HASH} *fuwa-desktop-0.5.0-x86_64-linux\n");
        assert!(matches!(
            judge(&manifest("0.5.0", &starred, &pair), "0.4.0", &[public(&pair)], Some("x86_64-linux")),
            Verdict::Install(_)
        ));
    }

    #[test]
    fn nothing_installs_without_a_signature_from_a_known_key() {
        let pair = key();
        let sums = format!("{HASH}  fuwa-desktop-0.5.0-x86_64-linux\n");
        let good = manifest("0.5.0", &sums, &pair);
        // No key built in yet, or an unsigned release: shown, never installed.
        assert_eq!(judge(&good, "0.4.0", &[], Some("x86_64-linux")), Verdict::Manual(Manual::Unsigned));
        let unsigned = Manifest { signature: String::new(), ..good.clone() };
        assert_eq!(
            judge(&unsigned, "0.4.0", &[public(&pair)], Some("x86_64-linux")),
            Verdict::Manual(Manual::Unsigned)
        );
        // Another key's signature, sums changed after signing, or garbage.
        let other = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[9u8; 32]).unwrap();
        assert_eq!(judge(&good, "0.4.0", &[public(&other)], Some("x86_64-linux")), Verdict::Forged);
        let swapped = Manifest { sums: sums.replace("ba78", "0000"), ..good.clone() };
        assert_eq!(judge(&swapped, "0.4.0", &[public(&pair)], Some("x86_64-linux")), Verdict::Forged);
        let garbage = Manifest { signature: "%%%".into(), ..good.clone() };
        assert_eq!(judge(&garbage, "0.4.0", &[public(&pair)], Some("x86_64-linux")), Verdict::Forged);
        // Any one of several keys will do (a key being rotated in).
        assert!(matches!(
            judge(&good, "0.4.0", &[public(&other), public(&pair)], Some("x86_64-linux")),
            Verdict::Install(_)
        ));
    }

    #[test]
    fn older_or_mislabelled_releases_never_install() {
        let pair = key();
        let sums = format!("{HASH}  fuwa-desktop-0.5.0-x86_64-linux\n");
        // Not newer: an old signed release handed back is ignored.
        assert_eq!(
            judge(&manifest("0.5.0", &sums, &pair), "0.5.0", &[public(&pair)], Some("x86_64-linux")),
            Verdict::UpToDate
        );
        assert_eq!(
            judge(&manifest("0.5.0", &sums, &pair), "0.6.0", &[public(&pair)], Some("x86_64-linux")),
            Verdict::UpToDate
        );
        // An instance claiming a newer version over an older release's sums finds no build of that version.
        assert_eq!(
            judge(&manifest("9.0.0", &sums, &pair), "0.5.0", &[public(&pair)], Some("x86_64-linux")),
            Verdict::Manual(Manual::NoBuild)
        );
        // No build for this computer.
        assert_eq!(
            judge(&manifest("0.5.0", &sums, &pair), "0.4.0", &[public(&pair)], None),
            Verdict::Manual(Manual::NoBuild)
        );
        assert_eq!(
            judge(&manifest("0.5.0", &sums, &pair), "0.4.0", &[public(&pair)], Some("aarch64-macos")),
            Verdict::Manual(Manual::NoBuild)
        );
    }

    #[test]
    fn signatures_from_the_release_workflow_check_out() {
        // Made as release.yml makes them: `openssl pkeyutl -sign -rawin` with an
        // Ed25519 key, the signature in base64, the public key's 32 raw bytes in base64.
        let keys = release_keys("zrsBnuNbYj1qtYobEUzlSCHu/qJTYjXnfnpRSLWJz80=");
        let signed = Manifest {
            version: "0.2.0".into(),
            notes: String::new(),
            sums: "aa  fuwa-desktop-0.2.0-x86_64-linux\n".into(),
            signature: "JSwu6VQxqqCDKaTZYOnvyj4afvUkkLyz6FKWw8ykTUwEU39vLaUv9UT4eMqP41oL+zvvr1YWMls1pYGmlFDjDg=="
                .into(),
            files: Vec::new(),
        };
        // Signed (its made-up hash just isn't a build to fetch).
        assert_eq!(judge(&signed, "0.1.0", &keys, Some("x86_64-linux")), Verdict::Manual(Manual::NoBuild));
        let changed = Manifest { sums: signed.sums.replace("aa", "ab"), ..signed };
        assert_eq!(judge(&changed, "0.1.0", &keys, Some("x86_64-linux")), Verdict::Forged);
    }

    #[test]
    fn release_keys_read_one_a_line() {
        let pair = key();
        let line = base64::engine::general_purpose::STANDARD.encode(public(&pair));
        let text = format!("# the release key\n\n{line}\nnot base64!\nAAAA\n");
        assert_eq!(release_keys(&text), vec![public(&pair)]);
        // The file the app is built with reads cleanly, whatever it holds.
        let _ = release_keys(RELEASE_KEYS);
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.2.0-rc.1", "0.1.0"));
        assert!(!newer("1.0", "0.1.0"));
    }

    #[test]
    fn updates_come_over_https_or_from_close_by() {
        assert!(courier_allowed("https://fuwa.chat"));
        assert!(courier_allowed("http://127.0.0.1:8787"));
        assert!(courier_allowed("http://localhost:8787"));
        assert!(courier_allowed("http://192.168.1.20"));
        assert!(courier_allowed("http://[::1]:8787"));
        assert!(courier_allowed("http://chat.local"));
        assert!(!courier_allowed("http://fuwa.chat"));
        assert!(!courier_allowed("http://8.8.8.8"));
        assert!(!courier_allowed("ftp://fuwa.chat"));
    }

    #[test]
    fn mac_app_bundles_update_by_hand() {
        assert!(in_bundle(Path::new("/Applications/fuwa.app/Contents/MacOS/fuwa-desktop")));
        assert!(!in_bundle(Path::new("/home/a/bin/fuwa-desktop")));
    }

    #[test]
    fn a_waiting_download_is_known_by_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir.path().join("fuwa-desktop"));
        assert_eq!(sha256_of(&part), None, "nothing waiting");
        std::fs::write(&part, b"abc").unwrap();
        let abc = hex32("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
        assert_eq!(sha256_of(&part), Some(abc));
        std::fs::write(&part, b"abd").unwrap();
        assert_ne!(sha256_of(&part), Some(abc), "a changed download isn't the checked one");
    }

    #[test]
    fn a_new_program_takes_the_old_ones_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fuwa-desktop");
        std::fs::write(&target, b"old").unwrap();
        let part = partial(&target);
        assert_eq!(part.file_name().unwrap(), ".fuwa-desktop.update");
        std::fs::write(&part, b"new").unwrap();
        put_in_place(&part, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!part.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        }
    }
}
