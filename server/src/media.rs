//! Pictures people upload: avatars, banners, server icons, emoji and backgrounds.
//!
//! An upload is reserved with `MediaService.CreateUpload`, which makes a row
//! in node.db's `media` table and a one-time token. The bytes arrive with an
//! HTTP PUT to `/media/upload/<token>`, and the picture is then served at
//! `/media/<id>` to anyone with the link (on a split instance, a server's
//! pictures are kept by its shard once it uses them; see
//! [`crate::cluster::pictures`]). Setting that link as a picture marks
//! it used; replacing or clearing it deletes the old one, and uploads nothing
//! uses are swept after a day.
//!
//! Files live in `<data>/media/`, named by id, as uploaded but without what
//! a picture says about where and how it was taken (a photo's GPS position,
//! camera and time: see `strip`). Pictures are public at their links, so they
//! aren't encrypted even when the databases are.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path as UrlPath};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use futures::StreamExt;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use tokio::io::AsyncWriteExt;
use ulid::Ulid;

use crate::app::App;
use crate::error::Result;
use crate::id::now_ms;
use crate::pb;

mod strip;

/// How long an upload link works.
pub const UPLOAD_TTL_MS: i64 = 10 * 60 * 1000;

/// How long an upload may take once its bytes start arriving.
pub const RECEIVE_TTL_MS: i64 = 60 * 60 * 1000;

/// How long a stored picture waits to be used before it's swept.
pub const UNUSED_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Backgrounds one account may keep (`MediaService.KeepBackground`).
pub const MAX_BACKGROUNDS: usize = 24;

/// Uploads one account may have reserved and not finished at once.
pub const MAX_PENDING_UPLOADS: i64 = 10;

/// What pictures can be, as `Content-Type`s.
pub const PICTURE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];

/// One uploaded file, as node.db keeps it.
#[derive(Debug, Clone)]
pub struct MediaRow {
    pub id: String,
    pub account_id: String,
    pub purpose: pb::MediaPurpose,
    pub content_type: String,
    pub size: i64,
    pub stored: bool,
    pub used: bool,
    pub server_id: Option<String>,
}

/// Where the files are.
pub struct Store {
    dir: PathBuf,
    incoming: PathBuf,
}

impl Store {
    /// Opens `<data>/media/`, clearing uploads a crash left half done.
    pub fn open(data: &Path) -> Result<Self> {
        let dir = data.join("media");
        let incoming = dir.join(".incoming");
        let _ = std::fs::remove_dir_all(&incoming);
        std::fs::create_dir_all(&incoming)?;
        Ok(Self { dir, incoming })
    }

    pub fn path(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// Deletes a file; one that's already gone is fine.
    pub fn remove(&self, id: &str) {
        if let Err(err) = std::fs::remove_file(self.path(id))
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(media = %id, error = %err, "couldn't delete an uploaded file");
        }
    }

    /// Deletes files no row points to, left by a crash between deleting a
    /// row and its file.
    pub fn remove_strays(&self, known: &HashSet<String>) -> Result<usize> {
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_file() && parse_id(&name).is_some() && !known.contains(&name) {
                self.remove(&name);
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// A new media id: 128 random bits, as a lowercase ULID string, so links
/// can't be guessed.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    Ulid::from(u128::from_le_bytes(bytes)).to_string().to_ascii_lowercase()
}

/// A media id from a request, in canonical form. Ids name files, so nothing
/// else gets through.
pub fn parse_id(value: &str) -> Option<String> {
    Ulid::from_string(value).ok().map(|id| id.to_string().to_ascii_lowercase())
}

/// The id in a link to one of this instance's pictures (`/media/<id>` on any
/// host, since the instance may be reached by more than one name).
pub fn id_in_url(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let mut segments = url.path_segments()?;
    match (segments.next(), segments.next(), segments.next()) {
        (Some("media"), Some(id), None) => parse_id(id),
        _ => None,
    }
}

/// What a file is, from its first bytes, if it's a picture fuwa takes.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if head.len() >= 12 && &head[4..8] == b"ftyp" && matches!(&head[8..12], b"avif" | b"avis") {
        Some("image/avif")
    } else {
        None
    }
}

/// Takes a JPEG's, PNG's or WebP's metadata out of the file at `path`, in
/// place, and says its new size; `None` for types kept as they came (GIF,
/// AVIF). A file whose metadata can't be found is an error, so it isn't kept
/// with it.
async fn without_metadata(path: &Path, content_type: &str) -> std::io::Result<Option<i64>> {
    if !matches!(content_type, "image/jpeg" | "image/png" | "image/webp") {
        return Ok(None);
    }
    let path = path.to_path_buf();
    let content_type = content_type.to_string();
    tokio::task::spawn_blocking(move || {
        let clean = path.with_extension("clean");
        let stripped = (|| {
            let mut from = std::io::BufReader::new(std::fs::File::open(&path)?);
            let mut to = std::io::BufWriter::new(std::fs::File::create(&clean)?);
            strip::strip(&content_type, &mut from, &mut to)?;
            let file = to.into_inner().map_err(|err| err.into_error())?;
            file.sync_all()?;
            Ok::<_, std::io::Error>(file.metadata()?.len())
        })();
        match stripped {
            Ok(size) => {
                std::fs::rename(&clean, &path)?;
                Ok(Some(size as i64))
            }
            Err(err) => {
                let _ = std::fs::remove_file(&clean);
                Err(err)
            }
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// "8 MB", "512 KB": a size for people.
pub fn size_label(bytes: i64) -> String {
    let bytes = bytes as f64;
    let (value, unit) = match bytes {
        b if b >= 1e9 => (b / 1e9, "GB"),
        b if b >= 1e6 => (b / 1e6, "MB"),
        b if b >= 1e3 => (b / 1e3, "KB"),
        b => (b, "bytes"),
    };
    let value = if value >= 10.0 || value.fract() < 0.05 { format!("{value:.0}") } else { format!("{value:.1}") };
    format!("{value} {unit}")
}

/// `PUT /media/upload/<token>` to send an upload's bytes, and `GET /media/<id>`
/// to fetch a picture.
pub fn routes(app: Arc<App>) -> Router {
    let uploads = app.clone();
    Router::new()
        .route("/media/{id}", get(move |id: UrlPath<String>, headers: HeaderMap| serve(app.clone(), id.0, headers)))
        .route(
            "/media/upload/{token}",
            put(move |token: UrlPath<String>, body: Body| upload(uploads.clone(), token.0, body))
                .layer(DefaultBodyLimit::disable()),
        )
}

/// node.db and the store. The routes are only served where pictures are kept.
fn kept(app: &App) -> (&crate::node::NodeDb, &Store) {
    match (app.node(), app.media()) {
        (Ok(node), Ok(media)) => (node, media),
        _ => unreachable!("picture routes are only served where pictures are kept"),
    }
}

fn plain(status: StatusCode, message: &str) -> Response {
    (status, format!("{message}\n")).into_response()
}

async fn serve(app: Arc<App>, id: String, headers: HeaderMap) -> Response {
    let Some(id) = parse_id(&id) else { return plain(StatusCode::NOT_FOUND, "not found") };
    let (node, media) = kept(&app);
    let row = match node.media(&id).await {
        Ok(Some(row)) if row.stored => row,
        Ok(_) => return plain(StatusCode::NOT_FOUND, "not found"),
        Err(err) => {
            tracing::error!(media = %id, error = %err, "couldn't look up an uploaded file");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server");
        }
    };
    if let Some(location) = crate::cluster::pictures::moved_to(&row, &app) {
        // Its server's shard keeps it now; the new link never changes either.
        let mut response = StatusCode::PERMANENT_REDIRECT.into_response();
        let h = response.headers_mut();
        h.insert(header::LOCATION, HeaderValue::from_str(&location).expect("ids are valid headers"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(PICTURE_CACHE));
        return response;
    }
    // A file never changes under its id, so the id is its version.
    let etag = format!("\"{id}\"");
    let fresh = headers.get(header::IF_NONE_MATCH).is_some_and(|value| value.as_bytes() == etag.as_bytes());
    let mut response = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        match tokio::fs::File::open(media.path(&id)).await {
            Ok(file) => Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file))),
            Err(_) => return plain(StatusCode::NOT_FOUND, "not found"),
        }
    };
    picture_headers(response.headers_mut(), &etag);
    let h = response.headers_mut();
    if !fresh {
        if let Ok(kind) = HeaderValue::from_str(&row.content_type) {
            h.insert(header::CONTENT_TYPE, kind);
        }
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(row.size));
    }
    response
}

/// How long pictures are kept: for good, since a file never changes under its
/// id, but only by the browser that fetched it. A shared cache (Railway's CDN
/// in front of fuwa.chat) would keep serving a picture after it's deleted,
/// a moderator's removal included.
pub const PICTURE_CACHE: &str = "private, max-age=31536000, immutable";

/// What every picture is served with: cached for good under its id (by
/// browsers only), shown on other sites (any fuwa client), but never run as
/// a page.
pub fn picture_headers(h: &mut HeaderMap, etag: &str) {
    h.insert(header::ETAG, HeaderValue::from_str(etag).expect("ids are valid headers"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(PICTURE_CACHE));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
}

async fn upload(app: Arc<App>, token: String, body: Body) -> Response {
    let hash = crate::auth::hash_token(&token);
    let (node, media) = kept(&app);
    let row = match node.start_upload(&hash, now_ms(), now_ms() + RECEIVE_TTL_MS).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return plain(StatusCode::NOT_FOUND, "this upload link has been used or ran out; start the upload again");
        }
        Err(err) => {
            tracing::error!(error = %err, "couldn't start an upload");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server");
        }
    };
    let temp = media.incoming.join(&row.id);
    let received =
        tokio::time::timeout(Duration::from_millis(RECEIVE_TTL_MS as u64), receive(&app, &row, &temp, body)).await;
    let failure = match received {
        Ok(Ok(())) => return StatusCode::NO_CONTENT.into_response(),
        Ok(Err(failure)) => failure,
        Err(_) => (StatusCode::REQUEST_TIMEOUT, "the upload took too long".to_string()),
    };
    let _ = tokio::fs::remove_file(&temp).await;
    if let Err(err) = node.delete_media(std::slice::from_ref(&row.id)).await {
        tracing::warn!(media = %row.id, error = %err, "couldn't drop a failed upload");
    }
    plain(failure.0, &failure.1)
}

/// Writes the body to a file beside the store, checks it, then moves it in.
async fn receive(app: &App, row: &MediaRow, temp: &Path, body: Body) -> std::result::Result<(), (StatusCode, String)> {
    let (content_type, size) = receive_file(&row.id, row.size, temp, body).await?;
    let (node, media) = kept(app);
    tokio::fs::rename(temp, media.path(&row.id)).await.map_err(|err| broken(&row.id, err))?;
    node.finish_upload(&row.id, content_type, size, now_ms()).await.map_err(|err| {
        media.remove(&row.id);
        tracing::error!(media = %row.id, error = %err, "couldn't record an upload");
        (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server".to_string())
    })
}

/// Writes an upload's body to `temp` and checks it: exactly `size` bytes of
/// a picture fuwa takes, with its metadata taken out (location, camera and
/// the like). Returns what it is and its size as kept. Every
/// upload goes through here, where accounts are kept or (a server's
/// pictures on a split instance) on its shard.
pub async fn receive_file(
    id: &str,
    size: i64,
    temp: &Path,
    body: Body,
) -> std::result::Result<(&'static str, i64), (StatusCode, String)> {
    let mut file = tokio::fs::File::create(temp).await.map_err(|err| broken(id, err))?;
    let mut stream = body.into_data_stream();
    let mut received: i64 = 0;
    let mut head = Vec::with_capacity(16);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| (StatusCode::BAD_REQUEST, "the upload was cut off".to_string()))?;
        received += chunk.len() as i64;
        if received > size {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("the file is bigger than the {} it was said to be", size_label(size)),
            ));
        }
        if head.len() < 16 {
            head.extend_from_slice(&chunk[..chunk.len().min(16 - head.len())]);
        }
        file.write_all(&chunk).await.map_err(|err| broken(id, err))?;
    }
    if received != size {
        return Err((StatusCode::BAD_REQUEST, format!("got {received} bytes of a {size}-byte file")));
    }
    let content_type = sniff(&head).ok_or_else(|| {
        (StatusCode::UNSUPPORTED_MEDIA_TYPE, "that isn't a PNG, JPEG, GIF, WebP or AVIF picture".to_string())
    })?;
    file.sync_all().await.map_err(|err| broken(id, err))?;
    drop(file);
    let kept = without_metadata(temp, content_type).await.map_err(|err| {
        tracing::info!(media = %id, error = %err, "refused an upload whose metadata couldn't be taken out");
        (StatusCode::UNPROCESSABLE_ENTITY, "that picture looks broken; save it again and upload that".to_string())
    })?;
    Ok((content_type, kept.unwrap_or(received)))
}

fn broken(id: &str, err: std::io::Error) -> (StatusCode, String) {
    tracing::error!(media = %id, error = %err, "couldn't store an upload");
    (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_and_parse_back() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 26);
        assert_eq!(parse_id(&a.to_ascii_uppercase()).as_deref(), Some(a.as_str()));
        assert!(parse_id("../node.db").is_none());
        assert!(parse_id(".incoming").is_none());
    }

    #[test]
    fn links_to_own_pictures_are_recognized_on_any_host() {
        let id = new_id();
        assert_eq!(id_in_url(&format!("https://chat.example.com/media/{id}")).as_deref(), Some(id.as_str()));
        assert_eq!(id_in_url(&format!("http://localhost:8080/media/{id}")).as_deref(), Some(id.as_str()));
        assert!(id_in_url(&format!("https://chat.example.com/media/{id}/more")).is_none());
        assert!(id_in_url("https://chat.example.com/media/upload").is_none());
        assert!(id_in_url("https://example.com/cat.png").is_none());
        assert!(id_in_url("not a url").is_none());
    }

    #[test]
    fn pictures_are_told_apart_by_their_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("image/png"));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0\0\x10JFIF"), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a\x01\0\x01\0"), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x24\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0"), Some("image/avif"));
        assert_eq!(sniff(b"<svg xmlns="), None);
        assert_eq!(sniff(b"<html><script>"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_label(8 * 1024 * 1024), "8.4 MB");
        assert_eq!(size_label(8_000_000), "8 MB");
        assert_eq!(size_label(512_000), "512 KB");
        assert_eq!(size_label(900), "900 bytes");
    }
}
