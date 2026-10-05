//! Files in channels shared between instances (docs/federation.md).
//!
//! The channel's home keeps every file sent in it. A guest server's person
//! uploads to their own instance as anywhere else; sending the message
//! gives the home a ticket for each file (32 random bytes, kept here only
//! as their SHA-256, for ten minutes, used once), and the home fetches the
//! file with a signed request (`GET /federation/files/<ticket>`) before it
//! writes the message. The guest's upload is then dropped.
//!
//! People on the guest's instance read the home's files through their own
//! instance, at `/media/shared/...` links it signs (with an expiry), which
//! fetch the file from the home (`GET /federation/attachments/<server>/<id>`,
//! signed too) each time: nothing is cached, so a deleted message stops
//! serving at once, and apps never talk to the other instance.
//!
//! Every fetch from another instance waits for one of a few slots
//! (InstanceSettings 52, 8 by default, each instance at most half), so
//! neither side can tie up this one. Only the part keeping node.db (one
//! process, or the directory) runs this; the shards pass bytes to it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, RawQuery};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::{Stream, StreamExt};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::app::{App, Link};
use crate::error::{Error, Result};
use crate::{cpb, federation, pb};

/// How long a ticket works.
const TICKET_TTL: Duration = Duration::from_secs(10 * 60);
/// Tickets kept at once, in all and for one instance: past these, the
/// oldest go (and their files have to be sent again). Internal bounds, so
/// a busy or hostile instance can't make this one hold more and more.
const MAX_TICKETS: usize = 4096;
const MAX_TICKETS_PER_INSTANCE: usize = 64;
/// How long a fetch waits for a slot before it gives up.
const SLOT_WAIT: Duration = Duration::from_secs(30);
/// How long a `/media/shared` link works: a day, at least, from when it was
/// made, and the same link all hour so browsers can keep it that long.
const LINK_HOURS: i64 = 24;
const HOUR_MS: i64 = 60 * 60 * 1000;
/// The bytes a file's kind is read from.
const HEAD: usize = crate::media::HEAD;

/// What one guest instance gave a home for a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    /// The guest server here the file was uploaded for, and by whom.
    pub server_id: String,
    pub account_id: String,
    pub media_id: String,
    /// The home's instance, the only one that may use it.
    pub origin: String,
    pub size: i64,
    expires: Instant,
}

#[derive(Default)]
struct Tickets {
    by_hash: HashMap<[u8; 32], Ticket>,
    order: VecDeque<[u8; 32]>,
}

/// What this instance keeps for files in channels shared with others.
#[derive(Default)]
pub struct State {
    tickets: Mutex<Tickets>,
    /// Fetches running now, in all and by instance.
    fetching: Mutex<(usize, HashMap<String, usize>)>,
    turn: tokio::sync::Notify,
}

fn hash(ticket: &str) -> [u8; 32] {
    Sha256::digest(ticket.as_bytes()).into()
}

impl State {
    /// A new ticket for a file the home at `origin` may fetch once.
    pub fn issue(&self, server_id: &str, account_id: &str, media_id: &str, origin: &str, size: i64) -> Result<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| Error::internal("the OS random number generator failed"))?;
        let ticket = URL_SAFE_NO_PAD.encode(bytes);
        let now = Instant::now();
        let mut tickets = self.tickets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Tickets { by_hash, order } = &mut *tickets;
        order.retain(|key| by_hash.get(key).is_some_and(|t| t.expires > now));
        by_hash.retain(|_, t| t.expires > now);
        let under = |by_hash: &HashMap<[u8; 32], Ticket>| by_hash.values().filter(|t| t.origin == origin).count();
        if under(by_hash) >= MAX_TICKETS_PER_INSTANCE
            && let Some(at) = order.iter().position(|key| by_hash.get(key).is_some_and(|t| t.origin == origin))
            && let Some(key) = order.remove(at)
        {
            by_hash.remove(&key);
        }
        while order.len() >= MAX_TICKETS {
            let Some(key) = order.pop_front() else { break };
            by_hash.remove(&key);
        }
        let key = hash(&ticket);
        by_hash.insert(
            key,
            Ticket {
                server_id: server_id.to_string(),
                account_id: account_id.to_string(),
                media_id: media_id.to_string(),
                origin: origin.to_string(),
                size,
                expires: now + TICKET_TTL,
            },
        );
        order.push_back(key);
        Ok(ticket)
    }

    /// Uses up a ticket, if it works and is for `origin`.
    pub fn take(&self, ticket: &str, origin: &str) -> Option<Ticket> {
        if ticket.len() != 43 {
            return None;
        }
        let key = hash(ticket);
        let mut tickets = self.tickets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if tickets.by_hash.get(&key).is_none_or(|t| t.origin != origin) {
            return None;
        }
        tickets.order.retain(|k| *k != key);
        tickets.by_hash.remove(&key).filter(|t| t.expires > Instant::now())
    }

    /// Forgets tickets the home didn't use: the message was answered.
    pub fn forget(&self, tickets: &[String]) {
        let mut all = self.tickets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for ticket in tickets {
            let key = hash(ticket);
            if all.by_hash.remove(&key).is_some() {
                all.order.retain(|k| *k != key);
            }
        }
    }

    fn try_slot(&self, origin: &str, limit: Option<usize>) -> bool {
        let mut fetching = self.fetching.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (all, by_origin) = &mut *fetching;
        if let Some(limit) = limit {
            let per_origin = (limit / 2).max(1);
            if *all >= limit || by_origin.get(origin).copied().unwrap_or(0) >= per_origin {
                return false;
            }
        }
        *all += 1;
        *by_origin.entry(origin.to_string()).or_default() += 1;
        true
    }

    fn free_slot(&self, origin: &str) {
        let mut fetching = self.fetching.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (all, by_origin) = &mut *fetching;
        *all = all.saturating_sub(1);
        if let Some(n) = by_origin.get_mut(origin) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                by_origin.remove(origin);
            }
        }
        drop(fetching);
        self.turn.notify_waiters();
    }
}

/// A fetch's turn: given back when it's dropped.
pub struct Slot {
    app: Arc<App>,
    origin: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.app.federation.files.free_slot(&self.origin);
    }
}

/// Waits for a turn to fetch from `origin`, for a while at most.
pub async fn slot(app: &Arc<App>, origin: &str) -> Result<Slot> {
    let state = &app.federation.files;
    let deadline = tokio::time::Instant::now() + SLOT_WAIT;
    loop {
        let turn = state.turn.notified();
        tokio::pin!(turn);
        turn.as_mut().enable();
        if state.try_slot(origin, app.settings().shared_file_fetches_in_flight()) {
            return Ok(Slot { app: app.clone(), origin: origin.to_string() });
        }
        if tokio::time::timeout_at(deadline, turn).await.is_err() {
            crate::reports::server_error("shared_files_busy", Some("shared_files"));
            return Err(Error::Unavailable("this instance is busy fetching files; try again in a minute".into()));
        }
    }
}

/// `GET /federation/files/<ticket>` and `/federation/attachments/<server>/<id>`
/// for other instances, and `GET /media/shared/<signature>?...` for people here.
pub fn routes(app: Arc<App>) -> Router {
    let (attachments, shared) = (app.clone(), app.clone());
    Router::new()
        .route(
            "/federation/files/{ticket}",
            get(move |UrlPath(ticket): UrlPath<String>, headers: HeaderMap| {
                let app = app.clone();
                async move { send_file(&app, &ticket, &headers).await.unwrap_or_else(not_found) }
            }),
        )
        .route(
            "/federation/attachments/{server_id}/{media_id}",
            get(move |UrlPath((server_id, media_id)): UrlPath<(String, String)>, headers: HeaderMap| {
                let app = attachments.clone();
                async move { send_attachment(&app, &server_id, &media_id, &headers).await.unwrap_or_else(not_found) }
            }),
        )
        .route(
            "/media/shared/{signature}",
            get(move |UrlPath(signature): UrlPath<String>, query: RawQuery| {
                let app = shared.clone();
                async move { serve(&app, &signature, query.0.as_deref().unwrap_or_default()).await }
            }),
        )
}

/// What every refusal to another instance is: one answer, so it can't
/// tell which check failed.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found\n").into_response()
}

fn plain(status: StatusCode, message: &str) -> Response {
    (status, format!("{message}\n")).into_response()
}

/// Bytes going to another instance, as one body of `size` bytes.
fn octets(size: i64, body: Body) -> Response {
    let mut response = Response::new(body);
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(crate::media::OCTET_STREAM));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Pieces from one of this instance's shards, as a body, cut off past `most` bytes.
fn pieces<S, T>(stream: S, most: i64, data: fn(T) -> Vec<u8>) -> Body
where
    S: Stream<Item = std::result::Result<T, tonic::Status>> + Send + 'static,
    T: Send + 'static,
{
    let mut sent = 0i64;
    Body::from_stream(stream.map(move |piece| {
        let bytes = data(piece.map_err(|_| std::io::Error::other("a part of this instance stopped sending"))?);
        sent += bytes.len() as i64;
        if sent > most {
            return Err(std::io::Error::other("more than the file's size"));
        }
        Ok::<_, std::io::Error>(Bytes::from(bytes))
    }))
}

/// A file's bytes from disk, `size` of them at most.
async fn from_disk(path: std::path::PathBuf, size: i64) -> Option<Body> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await.ok()?;
    let taken = file.take(u64::try_from(size).ok()?);
    Some(Body::from_stream(tokio_util::io::ReaderStream::new(taken)))
}

// ─────────────── The guest's instance: files going to a home ───────────────

/// Sends the home that holds a ticket the file it's for: only to that
/// home's instance, signed, once, while it works, and only a file that's
/// still that person's upload for that server, which no message has.
async fn send_file(app: &Arc<App>, ticket: &str, headers: &HeaderMap) -> Option<Response> {
    let path = format!("/federation/files/{ticket}");
    let origin = federation::signed_by(app, headers, &path).await?;
    let ticket = app.federation.files.take(ticket, &origin)?;
    match &app.link {
        Link::Directory(shards) => {
            let shard_id = app.index.placement(&ticket.server_id)?;
            let request = cpb::SendSharedFileRequest {
                server_id: ticket.server_id.clone(),
                media_id: ticket.media_id.clone(),
                home_id: String::new(),
                account_id: ticket.account_id.clone(),
            };
            let stream = shards.client(&shard_id).ok()?.send_shared_file(request).await.ok()?.into_inner();
            Some(octets(ticket.size, pieces(stream, ticket.size, |p: cpb::SendSharedFileResponse| p.data)))
        }
        _ => {
            let row = app.node().ok()?.media(&ticket.media_id).await.ok()??;
            let theirs = row.server_id.as_deref() == Some(ticket.server_id.as_str())
                && row.account_id == ticket.account_id
                && row.purpose == pb::MediaPurpose::Attachment
                && row.stored
                && !row.used
                && row.size == ticket.size;
            if !theirs {
                return None;
            }
            let body = from_disk(app.media().ok()?.path(&ticket.media_id), ticket.size).await?;
            Some(octets(ticket.size, body))
        }
    }
}

/// Gives the files of a message going to the home at `origin` their
/// tickets ([`State::issue`]), one for each in order. The attachments are
/// the guest's own uploads, checked as its people's are.
pub fn tickets_for(app: &App, send: &mut cpb::GuestSend, origin: &str) -> Result<Vec<String>> {
    let server_id = send.guest.as_ref().and_then(|g| g.server.as_ref()).map(|s| s.id.clone()).unwrap_or_default();
    let account_id = send.guest.as_ref().and_then(|g| g.user.as_ref()).map(|u| u.id.clone()).unwrap_or_default();
    let mut issued = Vec::with_capacity(send.attachments.len());
    for file in &mut send.attachments {
        let ticket = app.federation.files.issue(&server_id, &account_id, &file.id, origin, file.size)?;
        issued.push(ticket.clone());
        send.files.push(cpb::SharedFile { ticket });
        // The home makes its own link, under its own id.
        file.url.clear();
    }
    Ok(issued)
}

// ─────────────── The home's instance: files coming from a guest ───────────────

/// Reserves a row for the home's copy of a file a guest server on another
/// instance (`guest_server_id`, "<id>@<instance>") sends, counted toward
/// that server's bytes for the day, then fetches the file from there with
/// its `ticket`: exactly `size` bytes, no more. The row's id, and the
/// bytes. The row is the home's, used (by the message being written) but
/// not stored until the bytes are checked; one never finished is swept.
pub async fn fetch(
    app: &Arc<App>,
    home_id: &str,
    guest_server_id: &str,
    ticket: &str,
    size: i64,
) -> Result<(String, Slot, impl Stream<Item = std::io::Result<Bytes>> + Send + use<>)> {
    let address = guest_server_id.split_once('@').map(|(_, at)| at.to_string()).ok_or(Error::NotFound("server"))?;
    let origin = federation::origin(&address, app.federation.allows_private()).map_err(Error::invalid)?;
    if ticket.len() != 43 || !ticket.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(Error::invalid("that file's ticket doesn't read"));
    }
    let media_id = crate::media::new_id();
    let cap = app.settings().limits.shared_remote_file_bytes_per_day;
    let account = format!("shared:{guest_server_id}");
    app.node()?.reserve_shared_media(&media_id, &account, home_id, size, cap).await?;
    let fetched = async {
        let slot = slot(app, &origin).await?;
        let response = federation::signed_get(app, &address, &format!("/federation/files/{ticket}")).await?;
        if response.content_length().is_some_and(|length| length != size as u64) {
            return Err(Error::Unavailable(gone_wrong(&origin)));
        }
        Ok((slot, response))
    }
    .await;
    let (slot, response) = match fetched {
        Ok(fetched) => fetched,
        Err(err) => {
            let _ = app.node()?.delete_media(std::slice::from_ref(&media_id)).await;
            return Err(match err {
                Error::NotFound(_) => Error::NotFound("uploaded file; upload it again"),
                err => err,
            });
        }
    };
    let mut got = 0i64;
    let bytes = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(|_| std::io::Error::other("cut off"))?;
        got += chunk.len() as i64;
        if got > size {
            return Err(std::io::Error::other("more than the file's size"));
        }
        Ok(chunk)
    });
    Ok((media_id, slot, bytes))
}

fn gone_wrong(origin: &str) -> String {
    format!("{} sent a file that isn't what it said; try again", federation::display(origin))
}

/// Takes a file a guest server on another instance sends, in one process
/// or at the directory: fetched ([`fetch`]) into this instance's files, its
/// kind read from its bytes and its metadata taken out. The id and what
/// it is; nothing is left when it fails.
pub async fn take(
    app: &Arc<App>,
    home_id: &str,
    guest_server_id: &str,
    ticket: &str,
    size: i64,
) -> Result<(String, &'static str, i64)> {
    let (media_id, slot, bytes) = fetch(app, home_id, guest_server_id, ticket, size).await?;
    let node = app.node()?;
    let media = app.media()?;
    let temp = media.incoming(&media_id);
    let kept = async {
        crate::attachments::note_loose(app, home_id, &media_id).await?;
        let received = crate::media::receive_file(pb::MediaPurpose::Attachment, size, &temp, Body::from_stream(bytes));
        let (kind, kept) = received.await.map_err(|(_, why)| Error::Unavailable(why))?;
        drop(slot);
        tokio::fs::rename(&temp, media.path(&media_id)).await?;
        node.finish_upload(&media_id, kind, kept, crate::id::now_ms()).await?;
        Ok::<_, Error>((kind, kept))
    }
    .await;
    match kept {
        Ok((kind, kept)) => Ok((media_id, kind, kept)),
        Err(err) => {
            let _ = tokio::fs::remove_file(&temp).await;
            media.remove(&media_id);
            crate::attachments::forget_loose(app, home_id, &media_id).await;
            let _ = node.delete_media(std::slice::from_ref(&media_id)).await;
            Err(err)
        }
    }
}

/// Sends a file of a message here to a guest's instance, signed by it: only
/// one in a message shown in a channel its home shares with a server there.
async fn send_attachment(app: &Arc<App>, server_id: &str, media_id: &str, headers: &HeaderMap) -> Option<Response> {
    let server_id = crate::id::parse_id("server_id", server_id).ok()?;
    let media_id = crate::media::parse_id(media_id)?;
    let path = format!("/federation/attachments/{server_id}/{media_id}");
    let origin = federation::signed_by(app, headers, &path).await?;
    match &app.link {
        Link::Directory(shards) => {
            let shard_id = app.index.placement(&server_id)?;
            let request = cpb::SendSharedAttachmentRequest { server_id, media_id, origin };
            let mut stream = shards.client(&shard_id).ok()?.send_shared_attachment(request).await.ok()?.into_inner();
            // The first piece says how big it is.
            let first = stream.message().await.ok()??;
            let size = first.size;
            let first = cpb::SendSharedAttachmentResponse { data: first.data, size: 0 };
            let rest = futures::stream::once(async move { Ok(first) }).chain(stream);
            Some(octets(size, pieces(rest, size, |p: cpb::SendSharedAttachmentResponse| p.data)))
        }
        _ => {
            let (size, path) = attachment_file(app, &server_id, &media_id, &origin).await?;
            Some(octets(size, from_disk(path, size).await?))
        }
    }
}

/// A file a message of a server here has, for the instance at `origin`
/// (see [`send_attachment`]): its size and where it is, on the shard
/// holding the server or in one process.
pub async fn attachment_file(
    app: &App,
    server_id: &str,
    media_id: &str,
    origin: &str,
) -> Option<(i64, std::path::PathBuf)> {
    if !app.servers.holds(server_id) {
        return None;
    }
    let sdb = app.servers.get(server_id).await.ok()?;
    let file = crate::api::shared_file_for(&*sdb.read().ok()?, server_id, media_id, origin).await.ok()??;
    let path = match &app.link {
        Link::Shard(_) => app.config.data_path.join(crate::cluster::pictures::name(server_id, media_id)),
        _ => app.media().ok()?.path(media_id),
    };
    let size = tokio::fs::metadata(&path).await.ok()?.len();
    let size = i64::try_from(size).ok()?.min(file.size.max(0));
    Some((size, path))
}

// ─────────────── The guest's instance: reading the home's files ───────────────

/// What a `/media/shared` link names.
#[derive(Debug, PartialEq)]
struct Wanted {
    /// The home server, "<id>@<instance>".
    home: String,
    media_id: String,
    size: i64,
    name: String,
    until: i64,
}

impl Wanted {
    fn signature(&self, key: &crate::outside::Key) -> String {
        let mac = key.mac(&[
            b"fuwa shared file\0",
            self.home.as_bytes(),
            b"\0",
            self.media_id.as_bytes(),
            b"\0",
            self.size.to_string().as_bytes(),
            b"\0",
            self.until.to_string().as_bytes(),
            b"\0",
            self.name.as_bytes(),
        ]);
        mac[..16].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn read(query: &str) -> Option<Self> {
        let pairs = || url::form_urlencoded::parse(query.as_bytes());
        let get = |key: &str| pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned());
        Some(Self {
            home: get("home")?,
            media_id: get("id")?,
            size: get("size")?.parse().ok()?,
            name: get("name").unwrap_or_default(),
            until: get("until")?.parse().ok()?,
        })
    }
}

/// The link people here read a file of a home on another instance at
/// (`home_id`, "<id>@<instance>"), signed by this instance, for a day or so.
pub fn link(app: &App, home_id: &str, file: &pb::Attachment) -> String {
    let until = (crate::id::now_ms() / HOUR_MS + LINK_HOURS + 1) * HOUR_MS;
    let wanted = Wanted {
        home: home_id.to_string(),
        media_id: file.id.clone(),
        size: file.size,
        name: file.filename.clone(),
        until,
    };
    let encode = |text: &str| utf8_percent_encode(text, NON_ALPHANUMERIC).to_string();
    format!(
        "{}/media/shared/{}?home={}&id={}&size={}&until={}&name={}",
        app.settings().public_url.trim_end_matches('/'),
        wanted.signature(app.picture_key()),
        encode(&wanted.home),
        wanted.media_id,
        wanted.size,
        wanted.until,
        encode(&wanted.name),
    )
}

/// `GET /media/shared/<signature>?...`: a home's file fetched for someone
/// here, whole (no `Range`), its kind read again here from its first bytes
/// and served under this instance's rules: pictures shown, audio and video
/// played, anything else only downloaded. Never cached anywhere.
async fn serve(app: &Arc<App>, signature: &str, query: &str) -> Response {
    let Some(wanted) = Wanted::read(query) else { return not_found() };
    let signed = crate::auth::constant_time_eq(signature.as_bytes(), wanted.signature(app.picture_key()).as_bytes());
    if !signed || wanted.until <= crate::id::now_ms() || wanted.size <= 0 {
        return not_found();
    }
    let Some((server_id, address)) = wanted.home.split_once('@') else { return not_found() };
    let Ok(origin) = federation::origin(address, app.federation.allows_private()) else { return not_found() };
    let most = match app.settings().limits.attachment_upload_bytes {
        Some(cap) => wanted.size.min(cap),
        None => wanted.size,
    };
    let unreachable = || plain(StatusCode::BAD_GATEWAY, "can't reach that instance right now");
    let slot = match slot(app, &origin).await {
        Ok(slot) => slot,
        Err(_) => {
            return plain(StatusCode::SERVICE_UNAVAILABLE, "this instance is busy fetching files; try again soon");
        }
    };
    let path = format!("/federation/attachments/{server_id}/{}", wanted.media_id);
    let response = match federation::signed_get(app, address, &path).await {
        Ok(response) => response,
        Err(Error::NotFound(_)) => return not_found(),
        Err(_) => return unreachable(),
    };
    if response.content_length().is_none_or(|length| length > most as u64) {
        return unreachable();
    }
    let mut bytes = response.bytes_stream();
    let mut head = Vec::with_capacity(HEAD);
    let mut first = Vec::new();
    while head.len() < HEAD {
        match bytes.next().await {
            Some(Ok(chunk)) => {
                head.extend_from_slice(&chunk[..chunk.len().min(HEAD - head.len())]);
                first.push(chunk);
            }
            Some(Err(_)) => return unreachable(),
            None => break,
        }
    }
    let mut sent = 0i64;
    let body = futures::stream::iter(first.into_iter().map(Ok)).chain(bytes).map(move |chunk| {
        let _held = &slot;
        let chunk = chunk.map_err(|_| std::io::Error::other("cut off"))?;
        sent += chunk.len() as i64;
        if sent > most {
            return Err(std::io::Error::other("more than the file's size"));
        }
        Ok::<_, std::io::Error>(chunk)
    });
    let kind = crate::media::kind_of(&head);
    let mut response = Response::new(Body::from_stream(body));
    let h = response.headers_mut();
    let picture = crate::media::PICTURE_TYPES.contains(&kind);
    let shown = if picture || crate::media::playable(kind) { kind } else { crate::media::OCTET_STREAM };
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(shown));
    h.insert(
        header::CONTENT_DISPOSITION,
        crate::media::disposition(picture, &crate::attachments::clean_name(&wanted.name)),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickets_work_once_for_their_instance_only() {
        let state = State::default();
        let ticket = state.issue("s1", "a1", "m1", "https://home.example", 10).unwrap();
        assert_eq!(ticket.len(), 43);
        assert!(state.take(&ticket, "https://other.example").is_none(), "another instance can't use it");
        let taken = state.take(&ticket, "https://home.example").unwrap();
        assert_eq!((taken.media_id.as_str(), taken.size), ("m1", 10));
        assert!(state.take(&ticket, "https://home.example").is_none(), "used once");
        assert!(state.take("short", "https://home.example").is_none());
    }

    #[test]
    fn tickets_expire_and_are_capped() {
        let state = State::default();
        let ticket = state.issue("s1", "a1", "m1", "https://home.example", 10).unwrap();
        state.tickets.lock().unwrap().by_hash.values_mut().for_each(|t| t.expires = Instant::now());
        assert!(state.take(&ticket, "https://home.example").is_none(), "expired");

        let first = state.issue("s1", "a1", "m0", "https://busy.example", 1).unwrap();
        for i in 1..MAX_TICKETS_PER_INSTANCE {
            state.issue("s1", "a1", &format!("m{i}"), "https://busy.example", 1).unwrap();
        }
        let other = state.issue("s1", "a1", "x", "https://calm.example", 1).unwrap();
        state.issue("s1", "a1", "last", "https://busy.example", 1).unwrap();
        assert!(state.take(&first, "https://busy.example").is_none(), "the oldest of that instance went");
        assert!(state.take(&other, "https://calm.example").is_some(), "another instance's stayed");
        let all = state.tickets.lock().unwrap();
        assert_eq!(all.by_hash.len(), all.order.len());
    }

    #[test]
    fn forgotten_tickets_stop_working() {
        let state = State::default();
        let ticket = state.issue("s1", "a1", "m1", "https://home.example", 10).unwrap();
        state.forget(std::slice::from_ref(&ticket));
        assert!(state.take(&ticket, "https://home.example").is_none());
    }

    #[test]
    fn slots_are_shared_out_by_instance() {
        let state = State::default();
        assert!(state.try_slot("a", Some(4)));
        assert!(state.try_slot("a", Some(4)));
        assert!(!state.try_slot("a", Some(4)), "half of them for one instance");
        assert!(state.try_slot("b", Some(4)));
        assert!(state.try_slot("b", Some(4)));
        assert!(!state.try_slot("c", Some(4)), "all of them taken");
        state.free_slot("a");
        assert!(state.try_slot("c", Some(4)));
        assert!(state.try_slot("d", None), "no limit");
    }

    #[test]
    fn shared_links_read_back_and_are_signed_over_everything() {
        let key = crate::outside::Key::from_cluster_key("k");
        let wanted = Wanted {
            home: "01H@home.example".into(),
            media_id: "m1".into(),
            size: 10,
            name: "a b&c.png".into(),
            until: 5,
        };
        let encode = |text: &str| utf8_percent_encode(text, NON_ALPHANUMERIC).to_string();
        let query = format!("home={}&id=m1&size=10&until=5&name={}", encode(&wanted.home), encode(&wanted.name));
        let read = Wanted::read(&query).unwrap();
        assert_eq!(read, wanted);
        for changed in [
            Wanted { home: "01H@elsewhere.example".into(), ..Wanted::read(&query).unwrap() },
            Wanted { media_id: "m2".into(), ..Wanted::read(&query).unwrap() },
            Wanted { size: 11, ..Wanted::read(&query).unwrap() },
            Wanted { name: "a b&c.gif".into(), ..Wanted::read(&query).unwrap() },
            Wanted { until: 6, ..Wanted::read(&query).unwrap() },
        ] {
            assert_ne!(changed.signature(&key), wanted.signature(&key), "{changed:?}");
        }
    }
}
