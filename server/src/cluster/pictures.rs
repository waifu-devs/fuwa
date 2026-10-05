//! A split instance's server pictures (icons, emoji, webhooks' pictures)
//! live with their server, so they're kept in its region (docs/regions.md).
//!
//! Uploads arrive at the directory, as every upload does, and are served
//! there at `/media/<id>` until they're used. Once a server uses one, the
//! shard holding the server takes it ([`take`]): it copies the file from the
//! directory (checked by SHA-256) into `<data>/server-pictures/<server>/`
//! and its own replica (`server-pictures/<server>/<id>` in its bucket), then
//! the directory forgets its copy. From then on the picture's link
//! redirects to `/media/servers/<server>/<id>`, which gateways pass to the
//! shard holding the server, so its link never changes. The pictures go
//! with the server when it moves, as its recordings do; pictures a server
//! used before this, or that couldn't be taken, are taken when it moves.
//! When the server is deleted they're deleted with it, on its shard, in its
//! bucket and at the directory ([`drop_all`]).
//!
//! node.db keeps each picture's row (who uploaded it, its size and type),
//! as it keeps accounts. One process running everything keeps pictures
//! where it always did.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path as UrlPath};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use futures::Stream;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use tokio::sync::mpsc;
use tonic::Status;

use crate::app::{App, Link};
use crate::cpb;
use crate::error::{Error, MISROUTED, Result};
use crate::id::parse_id;
use crate::pb;

/// Where a shard keeps its servers' pictures, in its data folder and its bucket.
pub const DIR: &str = "server-pictures";
/// How much of a picture goes in one message.
const PIECE: usize = 1024 * 1024;

pub type PictureStream = Pin<Box<dyn Stream<Item = Result<cpb::SendPictureResponse, Status>> + Send>>;

/// A server's picture's path from the data folder, and its key in the bucket.
pub fn name(server_id: &str, media_id: &str) -> String {
    format!("{DIR}/{server_id}/{media_id}")
}

/// Where a server's pictures are kept on its shard.
pub fn server_dir(data: &Path, server_id: &str) -> PathBuf {
    data.join(DIR).join(server_id)
}

// ─────────────── The shard ───────────────

/// Takes a picture a server here uses from the directory. A picture that
/// isn't the server's to take (one an account or another server uses), or
/// was taken already, stays where it is.
pub async fn take(app: &App, server_id: &str, media_id: &str) -> Result<bool> {
    let Link::Shard(link) = &app.link else { return Ok(false) };
    let (server_id, media_id) = (parse_id("server_id", server_id)?, canonical(media_id)?);
    let data = &app.config.data_path;
    let dest = data.join(name(&server_id, &media_id));
    if dest.exists() {
        return Ok(false);
    }
    let request = cpb::SendPictureRequest { media_id: media_id.clone(), server_id: server_id.clone() };
    let mut stream = match link.directory().send_picture(request).await {
        Ok(stream) => stream.into_inner(),
        Err(status) if matches!(status.code(), tonic::Code::NotFound | tonic::Code::FailedPrecondition) => {
            return Ok(false);
        }
        Err(status) => return Err(status.into()),
    };
    let dir = server_dir(data, &server_id);
    std::fs::create_dir_all(&dir)?;
    let partial = dir.join(format!(".incoming-{media_id}"));
    receive(&mut stream, &partial, u64::MAX).await?;
    std::fs::rename(&partial, &dest)?;
    if let Some(replica) = app.servers.replica()
        && let Err(err) = replica.store().put_file(&name(&server_id, &media_id), &dest).await
    {
        // Kept at the directory until it's backed up here too.
        let _ = std::fs::remove_file(&dest);
        return Err(err);
    }
    let request = cpb::ForgetPictureRequest { media_id: media_id.clone(), server_id: server_id.clone() };
    link.ask(request, |mut d, r| async move { d.forget_picture(r).await }).await?;
    Ok(true)
}

/// Writes a picture's pieces to `partial`, checked against the SHA-256 the
/// last one carries and stopped past `most` bytes. Whatever went wrong,
/// nothing is left at `partial`.
async fn receive<T: Piece>(stream: &mut tonic::Streaming<T>, partial: &Path, most: u64) -> Result<()> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let received = async {
        let mut out = tokio::fs::File::create(partial).await?;
        let mut hash = Sha256::new();
        let mut got = 0u64;
        while let Some(piece) = stream.message().await.map_err(Error::retried)? {
            let (data, sha256) = piece.parts();
            got += data.len() as u64;
            if got > most {
                return Err(Error::internal("a picture was bigger than it should be"));
            }
            out.write_all(data).await?;
            hash.update(data);
            if sha256.is_empty() {
                continue;
            }
            out.sync_all().await?;
            if hash.finalize().as_slice() != sha256 {
                return Err(Error::internal("a picture didn't arrive intact"));
            }
            return Ok(());
        }
        Err(Error::internal("the other side stopped partway through a picture"))
    }
    .await;
    if received.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    received
}

/// A piece of a file sent between parts of the instance: its bytes, and
/// on the last one the whole file's SHA-256.
trait Piece: prost::Message + Default + 'static {
    fn parts(&self) -> (&[u8], &[u8]);
}

impl Piece for cpb::SendPictureResponse {
    fn parts(&self) -> (&[u8], &[u8]) {
        (&self.data, &self.sha256)
    }
}

impl Piece for cpb::SendSharedFileResponse {
    fn parts(&self) -> (&[u8], &[u8]) {
        (&self.data, &self.sha256)
    }
}

/// Takes in the background, after a server here started using a picture.
/// One that fails stays at the directory, served from there, and is taken
/// when the server next moves.
pub fn take_soon(app: Arc<App>, server_id: String, media_id: String) {
    tokio::spawn(async move {
        if take(&app, &server_id, &media_id).await.is_err() {
            tracing::warn!("couldn't take a server's picture");
            crate::reports::server_error("server_picture_take", Some("cluster::pictures"));
        }
    });
}

/// Takes every picture of a server here that the directory still keeps.
pub async fn take_all(app: &App, server_id: &str) -> Result<usize> {
    let Link::Shard(link) = &app.link else { return Ok(0) };
    let request = cpb::ServerPicturesRequest { server_id: server_id.to_string() };
    let ids = link.ask(request, |mut d, r| async move { d.server_pictures(r).await }).await?.media_ids;
    let mut taken = 0;
    for id in ids {
        taken += usize::from(take(app, server_id, &id).await?);
    }
    Ok(taken)
}

/// Deletes a server's picture here, and its replica's copy: it was replaced
/// or removed. One that isn't here is fine.
pub async fn drop(app: &App, server_id: &str, media_id: &str) {
    let (Ok(server_id), Ok(media_id)) = (parse_id("server_id", server_id), canonical(media_id)) else { return };
    let key = name(&server_id, &media_id);
    match std::fs::remove_file(app.config.data_path.join(&key)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("couldn't delete a server's picture");
        }
        // A file no message had is gone now, so it needn't be noted.
        _ if app.servers.holds(&server_id) => crate::attachments::forget_loose(app, &server_id, &media_id).await,
        _ => {}
    }
    if let Some(replica) = app.servers.replica()
        && let Err(_) = replica.store().delete(&key).await
    {
        tracing::warn!("couldn't delete a server's picture from the replica");
    }
}

/// Deletes every picture of a deleted server here, and the replica's copies.
/// What can't be deleted is logged and reported; nothing comes back for it.
pub async fn drop_all(app: &App, server_id: &str) {
    let Ok(server_id) = parse_id("server_id", server_id) else { return };
    match std::fs::remove_dir_all(server_dir(&app.config.data_path, &server_id)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("couldn't delete a deleted server's pictures");
            crate::reports::server_error("server_pictures_drop", Some("cluster::pictures"));
        }
        _ => {}
    }
    let Some(replica) = app.servers.replica() else { return };
    let prefix = format!("{DIR}/{server_id}/");
    let dropped = async {
        for object in replica.store().list(&prefix).await? {
            replica.store().delete(&object.key).await?;
        }
        Ok::<_, Error>(())
    };
    if dropped.await.is_err() {
        tracing::warn!("couldn't delete a deleted server's pictures from the replica");
        crate::reports::server_error("server_pictures_drop_replica", Some("cluster::pictures"));
    }
}

/// Copies a server's pictures here up to the replica: it was just adopted.
pub async fn replicate(app: &App, server_id: &str) -> Result<()> {
    let Some(replica) = app.servers.replica() else { return Ok(()) };
    for (file, path) in pictures(&app.config.data_path, server_id)? {
        replica.store().put_file(&name(server_id, &file), &path).await?;
    }
    Ok(())
}

/// Fetches any of a server's pictures this shard lost back from its replica,
/// so a move sends them all.
pub async fn restore_all(app: &App, server_id: &str) -> Result<()> {
    let Some(replica) = app.servers.replica() else { return Ok(()) };
    let prefix = format!("{DIR}/{server_id}/");
    for object in replica.store().list(&prefix).await? {
        let Some(id) = object.key.strip_prefix(&prefix).filter(|id| is_id(id)) else { continue };
        let path = app.config.data_path.join(name(server_id, id));
        if !path.exists() && !restore(app, &object.key, &path).await {
            return Err(Error::internal(format!("couldn't fetch picture {id} from the replica")));
        }
    }
    Ok(())
}

/// A server's pictures here, by id.
pub fn pictures(data: &Path, server_id: &str) -> Result<Vec<(String, PathBuf)>> {
    let entries = match std::fs::read_dir(server_dir(data, server_id)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(file) = entry.file_name().into_string() else { continue };
        if entry.file_type()?.is_file() && is_id(&file) {
            found.push((file, entry.path()));
        }
    }
    found.sort();
    Ok(found)
}

/// `GET /media/servers/<server>/<id>`, where a picture's link leads once its
/// server's shard has it, and `PUT /media/servers/<server>/upload/<token>`,
/// where a picture uploaded for the server arrives.
pub fn routes(app: Arc<App>) -> Router {
    let uploads = app.clone();
    Router::new()
        .route(
            "/media/servers/{server_id}/{id}",
            get(move |UrlPath((server_id, id)): UrlPath<(String, String)>, headers: HeaderMap| {
                let app = app.clone();
                async move { serve(&app, &server_id, &id, &headers).await }
            }),
        )
        .route(
            "/media/servers/{server_id}/upload/{token}",
            put(move |UrlPath((server_id, token)): UrlPath<(String, String)>, body: Body| {
                let app = uploads.clone();
                async move { upload(&app, &server_id, &token, body).await }
            })
            .layer(DefaultBodyLimit::disable()),
        )
}

/// Receives a picture uploaded for a server here: its bytes are kept with
/// the server and its replica, never at the directory.
async fn upload(app: &App, server_id: &str, token: &str, body: Body) -> Response {
    let Ok(server_id) = parse_id("server_id", server_id) else { return plain(StatusCode::NOT_FOUND, "not found") };
    let Link::Shard(link) = &app.link else { return plain(StatusCode::NOT_FOUND, "not found") };
    if !app.servers.holds(&server_id) {
        return misrouted();
    }
    let request =
        cpb::StartServerUploadRequest { upload_hash: crate::auth::hash_token(token), server_id: server_id.clone() };
    let started = match link.directory().start_server_upload(request).await {
        Ok(started) => started.into_inner(),
        Err(status) if status.code() == tonic::Code::NotFound => {
            return plain(StatusCode::NOT_FOUND, "this upload link has been used or ran out; start the upload again");
        }
        Err(_) => {
            tracing::warn!("couldn't start a server picture's upload");
            return plain(StatusCode::BAD_GATEWAY, "part of this instance is unreachable right now; try again soon");
        }
    };
    let id = match canonical(&started.media_id) {
        Ok(id) => id,
        Err(_) => return plain(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server"),
    };
    let dir = server_dir(&app.config.data_path, &server_id);
    let temp = dir.join(format!(".incoming-{id}"));
    let dest = dir.join(&id);
    let purpose = pb::MediaPurpose::try_from(started.purpose).unwrap_or(pb::MediaPurpose::Unspecified);
    let kept = async {
        std::fs::create_dir_all(&dir).map_err(|_| failed_io())?;
        let wait = std::time::Duration::from_millis(crate::media::RECEIVE_TTL_MS as u64);
        let received = crate::media::receive_file(purpose, started.size, &temp, body);
        let (kind, size) = tokio::time::timeout(wait, received)
            .await
            .map_err(|_| (StatusCode::REQUEST_TIMEOUT, "the upload took too long".to_string()))??;
        // A file is loose until a message takes it, and never served before.
        if purpose == pb::MediaPurpose::Attachment
            && crate::attachments::note_loose(app, &server_id, &id).await.is_err()
        {
            tracing::warn!("couldn't note an uploaded file");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server".to_string()));
        }
        std::fs::rename(&temp, &dest).map_err(|_| failed_io())?;
        if let Some(replica) = app.servers.replica() {
            replica.store().put_file(&name(&server_id, &id), &dest).await.map_err(|_| {
                tracing::error!("couldn't back up an uploaded picture");
                (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server".to_string())
            })?;
        }
        // A move that started meanwhile has already sent the server's
        // pictures, so this one wouldn't go with it.
        if !app.servers.holds(&server_id) || app.servers.frozen().contains(&server_id) {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "that server is moving; upload it again in a moment".into()));
        }
        Ok::<_, (StatusCode, String)>((kind, size))
    }
    .await;
    let (status, message, finish) = match kept {
        Ok((kind, size)) => (StatusCode::NO_CONTENT, String::new(), Some((kind, size))),
        Err((status, message)) => (status, message, None),
    };
    let request = cpb::FinishServerUploadRequest {
        media_id: id.clone(),
        server_id: server_id.clone(),
        content_type: finish.map(|(kind, _)| kind.to_string()).unwrap_or_default(),
        size: finish.map_or(0, |(_, size)| size),
    };
    let finished = link.ask(request, |mut d, r| async move { d.finish_server_upload(r).await }).await;
    if finish.is_some() && finished.is_ok() {
        return status.into_response();
    }
    let _ = std::fs::remove_file(&temp);
    drop(app, &server_id, &id).await;
    if finished.is_err() {
        tracing::warn!("couldn't record a server picture's upload");
        if finish.is_some() {
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server");
        }
    }
    plain(status, &message)
}

fn failed_io() -> (StatusCode, String) {
    tracing::error!("couldn't store an uploaded picture");
    (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server".to_string())
}

fn misrouted() -> Response {
    let mut response = plain(StatusCode::SERVICE_UNAVAILABLE, "that server is on another shard");
    response.headers_mut().insert(MISROUTED, HeaderValue::from_static("1"));
    response
}

/// How long a picture uploaded here waits to be used before it's deleted:
/// after the directory has swept its row.
const UNUSED_FOR_MS: i64 = 2 * crate::media::UNUSED_TTL_MS;

/// Hourly: deletes pictures uploaded for this shard's servers that nothing
/// uses.
pub fn spawn_sweep(app: Arc<App>) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        loop {
            tokio::select! {
                _ = app.shutdown.cancelled() => return,
                _ = every.tick() => match sweep(&app, crate::id::now_ms()).await {
                    Ok(0) => {}
                    Ok(swept) => tracing::info!(swept, "deleted server pictures nothing used"),
                    Err(_) => tracing::warn!("couldn't sweep server pictures"),
                },
            }
        }
    });
}

/// Deletes pictures of servers here, as of `now`, that the server doesn't
/// use and that arrived long enough ago that they never will be.
pub async fn sweep(app: &App, now: i64) -> Result<usize> {
    let entries = match std::fs::read_dir(app.config.data_path.join(DIR)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let mut swept = 0;
    for entry in entries {
        let Ok(server_id) = entry?.file_name().into_string() else { continue };
        if !app.servers.holds(&server_id) || app.servers.frozen().contains(&server_id) {
            continue;
        }
        for (id, path) in pictures(&app.config.data_path, &server_id)? {
            let arrived = std::fs::metadata(&path)?.modified()?;
            let arrived = arrived.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
            if now - arrived >= UNUSED_FOR_MS && unused(uses(app, &server_id, &id).await) {
                drop(app, &server_id, &id).await;
                swept += 1;
            }
        }
    }
    Ok(swept)
}

async fn serve(app: &App, server_id: &str, id: &str, headers: &HeaderMap) -> Response {
    let (Ok(server_id), Ok(id)) = (parse_id("server_id", server_id), canonical(id)) else {
        return plain(StatusCode::NOT_FOUND, "not found");
    };
    if !app.servers.holds(&server_id) {
        return misrouted();
    }
    let key = name(&server_id, &id);
    let path = app.config.data_path.join(&key);
    // Only a picture the server uses is fetched back, so asking for made-up
    // ids never reaches the bucket.
    if !path.exists() && !(matches!(uses(app, &server_id, &id).await, Ok(true)) && restore(app, &key, &path).await) {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    // A message's file is served while a message has it, by what it is.
    match crate::attachments::find(app, &server_id, &id).await {
        Ok(Some(file)) => return crate::media::serve_file(&path, &file, headers).await,
        // A file no message has (yet, or any more) is never served, whatever
        // its bytes look like; anything else is a picture.
        Ok(None) => {
            match async { crate::attachments::is_loose(&*app.servers.get(&server_id).await?.read()?, &id).await }.await
            {
                Ok(false) => {}
                Ok(true) => return plain(StatusCode::NOT_FOUND, "not found"),
                Err(_) => {
                    tracing::warn!("couldn't look up an attachment");
                    return plain(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "that file can't be reached right now; try again soon",
                    );
                }
            }
        }
        Err(_) => {
            tracing::warn!("couldn't look up an attachment");
            return plain(StatusCode::SERVICE_UNAVAILABLE, "that file can't be reached right now; try again soon");
        }
    }
    let etag = format!("\"{id}\"");
    let fresh = headers.get(header::IF_NONE_MATCH).is_some_and(|value| value.as_bytes() == etag.as_bytes());
    let mut response = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        // Only its first bytes are read to tell what it is, so a big file
        // (an attachment no message has yet) never sits in memory.
        let Ok(mut file) = tokio::fs::File::open(&path).await else { return plain(StatusCode::NOT_FOUND, "not found") };
        let mut head = [0u8; 16];
        let mut read = 0;
        while read < head.len() {
            match tokio::io::AsyncReadExt::read(&mut file, &mut head[read..]).await {
                Ok(0) => break,
                Ok(n) => read += n,
                Err(_) => return plain(StatusCode::NOT_FOUND, "not found"),
            }
        }
        let Some(kind) = crate::media::sniff(&head[..read]) else {
            return plain(StatusCode::NOT_FOUND, "not found");
        };
        if tokio::io::AsyncSeekExt::rewind(&mut file).await.is_err() {
            return plain(StatusCode::NOT_FOUND, "not found");
        }
        let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
        response
    };
    crate::media::picture_headers(response.headers_mut(), &etag);
    response
}

/// Whether a picture is certainly unused, so the sweep may delete it. When
/// the server couldn't be asked (it's busy, restarting or moving), it's
/// kept: only a definite no deletes anything.
fn unused(used: Result<bool>) -> bool {
    match used {
        Ok(used) => !used,
        Err(_) => {
            tracing::warn!("couldn't tell whether a server uses a picture; keeping it");
            false
        }
    }
}

/// Whether the server links to the file: its icon, an emoji, a webhook's
/// picture or a message's attachment.
pub(crate) async fn uses(app: &App, server_id: &str, media_id: &str) -> Result<bool> {
    let used = async {
        let sdb = app.servers.get(server_id).await?;
        let conn = sdb.read()?;
        // Ids are letters and digits only, so nothing in one is a wildcard.
        let link = format!("%/media/{media_id}");
        crate::db::query_one(
            &conn,
            "SELECT 1 FROM server WHERE icon_url LIKE ?1
             UNION ALL SELECT 1 FROM emojis WHERE url LIKE ?1
             UNION ALL SELECT 1 FROM webhooks WHERE avatar_url LIKE ?1
             UNION ALL SELECT 1 FROM attachments WHERE media_id = ?2 LIMIT 1",
            (link.as_str(), media_id),
            |r| r.get::<i64>(0),
        )
        .await
    }
    .await;
    used.map(|found| found.is_some())
}

/// Fetches a picture this shard lost (its disk was replaced) back from its
/// replica.
async fn restore(app: &App, key: &str, path: &Path) -> bool {
    let Some(replica) = app.servers.replica() else { return false };
    let Some(dir) = path.parent() else { return false };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let partial = dir.join(format!(".restoring-{}", path.file_name().and_then(|n| n.to_str()).unwrap_or_default()));
    match replica.store().get_to_file(key, &partial).await {
        Ok(true) => std::fs::rename(&partial, path).is_ok(),
        Ok(false) => false,
        Err(_) => {
            tracing::warn!("couldn't fetch a server's picture from the replica");
            let _ = std::fs::remove_file(&partial);
            false
        }
    }
}

fn plain(status: StatusCode, message: &str) -> Response {
    (status, format!("{message}\n")).into_response()
}

// ─────────────── The directory ───────────────

/// Whether the picture is the server's to take: an icon, banner, emoji or
/// webhook picture it uses, still kept here.
async fn takeable(app: &App, server_id: &str, media_id: &str) -> Result<crate::media::MediaRow> {
    let row = app.node()?.media(media_id).await?.ok_or(Error::NotFound("picture"))?;
    let purpose = matches!(
        row.purpose,
        pb::MediaPurpose::ServerIcon
            | pb::MediaPurpose::Banner
            | pb::MediaPurpose::Emoji
            | pb::MediaPurpose::Avatar
            | pb::MediaPurpose::Attachment
    );
    if !row.stored || !row.used || !purpose || row.server_id.as_deref() != Some(server_id) {
        return Err(Error::FailedPrecondition("that picture isn't the server's".into()));
    }
    Ok(row)
}

/// Uses up the link for a picture uploaded for `server_id`, whose bytes its
/// shard is receiving. A link for another server's picture is used up for
/// nothing.
pub async fn start_upload(app: &App, upload_hash: &str, server_id: &str) -> Result<cpb::StartServerUploadResponse> {
    let node = app.node()?;
    let now = crate::id::now_ms();
    let row = node
        .start_upload(upload_hash, now, now + crate::media::RECEIVE_TTL_MS)
        .await?
        .ok_or(Error::NotFound("upload link"))?;
    if row.server_id.as_deref() != Some(server_id) {
        app.delete_media(std::slice::from_ref(&row.id)).await?;
        return Err(Error::NotFound("upload link"));
    }
    Ok(cpb::StartServerUploadResponse { media_id: row.id, size: row.size, purpose: row.purpose as i32 })
}

/// Records a server picture's upload its shard received, or drops it
/// (`content_type` is `None`) when it didn't arrive whole.
pub async fn finish_upload(
    app: &App,
    media_id: &str,
    server_id: &str,
    content_type: Option<&str>,
    size: i64,
) -> Result<()> {
    let node = app.node()?;
    let row = node.media(media_id).await?.ok_or(Error::NotFound("upload"))?;
    if row.server_id.as_deref() != Some(server_id) || row.stored {
        return Err(Error::FailedPrecondition("that isn't an upload for that server in progress".into()));
    }
    match content_type {
        // Taking out its metadata only ever makes it smaller.
        Some(kind) if kinds(row.purpose).contains(&kind) && (1..=row.size).contains(&size) => {
            node.finish_upload(&row.id, kind, size, crate::id::now_ms()).await
        }
        Some(_) => Err(Error::invalid("that isn't the picture that was reserved")),
        None => app.delete_media(std::slice::from_ref(&row.id)).await,
    }
}

/// What an upload for this purpose may turn out to be.
fn kinds(purpose: pb::MediaPurpose) -> &'static [&'static str] {
    match purpose {
        pb::MediaPurpose::Attachment => crate::media::ATTACHMENT_TYPES,
        _ => crate::media::PICTURE_TYPES,
    }
}

/// Sends a server's picture to its shard.
pub async fn send(
    app: Arc<App>,
    server_id: String,
    media_id: String,
) -> Result<mpsc::Receiver<Result<cpb::SendPictureResponse, Status>>> {
    let media_id = canonical(&media_id)?;
    takeable(&app, &server_id, &media_id).await?;
    let file = match tokio::fs::File::open(app.media()?.path(&media_id)).await {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::FailedPrecondition("that picture was taken already".into()));
        }
        Err(err) => return Err(err.into()),
    };
    Ok(stream_file(file))
}

/// A file's pieces, the last carrying its SHA-256.
fn stream_file(mut file: tokio::fs::File) -> mpsc::Receiver<Result<cpb::SendPictureResponse, Status>> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;

    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        let mut hash = Sha256::new();
        let mut buffer = vec![0; PIECE];
        loop {
            let piece = match file.read(&mut buffer).await {
                Ok(0) => {
                    cpb::SendPictureResponse { data: Vec::new(), sha256: std::mem::take(&mut hash).finalize().to_vec() }
                }
                Ok(read) => {
                    hash.update(&buffer[..read]);
                    cpb::SendPictureResponse { data: buffer[..read].to_vec(), sha256: Vec::new() }
                }
                Err(err) => {
                    let _ = tx.send(Err(Error::from(err).into())).await;
                    return;
                }
            };
            let last = !piece.sha256.is_empty();
            if tx.send(Ok(piece)).await.is_err() || last {
                return;
            }
        }
    });
    rx
}

// ─────────────── Files sent in a shared channel ───────────────

/// Sends a file a server here uploaded and no message has yet to the shard
/// holding the shared channel's home that took it ([`take_shared`]): only
/// one still loose here whose row says that home took it from `account_id`.
pub async fn send_shared(app: &App, req: &cpb::SendSharedFileRequest) -> Result<SharedFileStream> {
    let (server_id, media_id) = (parse_id("server_id", &req.server_id)?, canonical(&req.media_id)?);
    let home_id = parse_id("server_id", &req.home_id)?;
    if !app.servers.holds(&server_id) {
        return Err(Error::Misrouted);
    }
    if !crate::attachments::is_loose(&*app.servers.get(&server_id).await?.read()?, &media_id).await? {
        return Err(Error::NotFound(GONE_UPLOAD));
    }
    let url = format!("{}/media/{media_id}", app.settings().public_url);
    match app.check_upload(&req.account_id, pb::MediaPurpose::Attachment, &url, Some(&home_id)).await {
        Ok(Some(_)) => {}
        _ => return Err(Error::NotFound(GONE_UPLOAD)),
    }
    match tokio::fs::File::open(app.config.data_path.join(name(&server_id, &media_id))).await {
        Ok(file) => {
            use tokio_stream::StreamExt;
            let pieces = tokio_stream::wrappers::ReceiverStream::new(stream_file(file));
            let pieces =
                pieces.map(|piece| piece.map(|p| cpb::SendSharedFileResponse { data: p.data, sha256: p.sha256 }));
            Ok(Box::pin(pieces))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound(GONE_UPLOAD)),
        Err(err) => Err(err.into()),
    }
}

pub type SharedFileStream = Pin<Box<dyn Stream<Item = Result<cpb::SendSharedFileResponse, Status>> + Send>>;

/// What's said of an upload that's no longer there to send.
pub const GONE_UPLOAD: &str = "uploaded file; upload it again";

/// Puts a file a guest server on this instance uploaded with the home
/// server here, once the home has claimed its row
/// ([`App::take_attachment`]) and before the message that has it is
/// written: noted loose first, so it's never served before a message has
/// it, then moved over (the guest's on this shard too) or copied from the
/// guest's shard, at most `size` bytes, and backed up. In one process the
/// file stays where it is.
pub async fn take_shared(
    app: &App,
    guest_id: &str,
    home_id: &str,
    media_id: &str,
    account_id: &str,
    size: u64,
) -> Result<()> {
    let Link::Shard(link) = &app.link else { return Ok(()) };
    let (guest_id, home_id, media_id) =
        (parse_id("server_id", guest_id)?, parse_id("server_id", home_id)?, canonical(media_id)?);
    if app.servers.frozen().contains(&home_id) {
        return Err(Error::Moving);
    }
    crate::attachments::note_loose(app, &home_id, &media_id).await?;
    let data = &app.config.data_path;
    let dir = server_dir(data, &home_id);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(&media_id);
    if app.servers.holds(&guest_id) {
        match std::fs::rename(data.join(name(&guest_id, &media_id)), &dest) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && !dest.exists() => {
                return Err(Error::NotFound(GONE_UPLOAD));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    } else if !dest.exists() {
        let request = cpb::PlacementsRequest { server_ids: vec![guest_id.clone()] };
        let placements = link.ask(request, |mut d, r| async move { d.placements(r).await }).await?;
        let url = placements
            .placements
            .into_iter()
            .find(|p| p.server_id == guest_id && !p.url.is_empty())
            .map(|p| p.url)
            .ok_or_else(|| {
                Error::Unavailable("part of this instance is unreachable right now; try again soon".into())
            })?;
        let request = cpb::SendSharedFileRequest {
            server_id: guest_id.clone(),
            media_id: media_id.clone(),
            home_id: home_id.clone(),
            account_id: account_id.to_string(),
        };
        let mut stream = super::shard_client(&url, app.config.cluster.key_value()?)?
            .send_shared_file(request)
            .await
            .map_err(|status| match status.code() {
                tonic::Code::NotFound => Error::NotFound(GONE_UPLOAD),
                _ => Error::retried(status),
            })?
            .into_inner();
        let partial = dir.join(format!(".incoming-{media_id}"));
        receive(&mut stream, &partial, size).await?;
        std::fs::rename(&partial, &dest)?;
    }
    if let Some(replica) = app.servers.replica() {
        replica.store().put_file(&name(&home_id, &media_id), &dest).await?;
    }
    Ok(())
}

/// Deletes the directory's copy of a picture its server's shard has taken.
pub async fn forget(app: &App, server_id: &str, media_id: &str) -> Result<()> {
    let media_id = canonical(media_id)?;
    takeable(app, server_id, &media_id).await?;
    app.media()?.remove(&media_id);
    if let Some(replica) = &app.replica {
        replica.drop_media(std::slice::from_ref(&media_id)).await;
    }
    Ok(())
}

/// A server's pictures still kept here.
pub async fn kept_here(app: &App, server_id: &str) -> Result<Vec<String>> {
    let server_id = parse_id("server_id", server_id)?;
    let media = app.media()?;
    let ids = app.node()?.server_media(&server_id).await?;
    Ok(ids.into_iter().filter(|id| media.path(id).exists()).collect())
}

/// Where a picture's link leads when the directory no longer has it: the
/// shard holding its server.
pub fn moved_to(row: &crate::media::MediaRow, app: &App) -> Option<String> {
    let server_id = row.server_id.as_deref()?;
    let taken = matches!(app.link, Link::Directory(_)) && !app.media().ok()?.path(&row.id).exists();
    taken.then(|| format!("/media/servers/{server_id}/{}", row.id))
}

fn canonical(media_id: &str) -> Result<String> {
    crate::media::parse_id(media_id)
        .filter(|id| id == media_id)
        .ok_or_else(|| Error::invalid("that isn't a picture's id"))
}

fn is_id(file: &str) -> bool {
    crate::media::parse_id(file).as_deref() == Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_definite_no_sweeps_a_picture() {
        assert!(unused(Ok(false)));
        assert!(!unused(Ok(true)));
        for failed in [Error::Busy, Error::Misrouted, Error::Moving, Error::internal("the read failed")] {
            assert!(!unused(Err(failed)));
        }
    }

    #[test]
    fn only_picture_ids_name_files() {
        let id = crate::media::new_id();
        assert!(is_id(&id));
        assert!(canonical(&id).is_ok());
        for bad in ["", "..", ".incoming-x", "a/b", &id.to_uppercase()] {
            assert!(canonical(bad).is_err(), "{bad}");
        }
    }
}
