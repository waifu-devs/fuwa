//! Where sendable GIFs are kept: node.db's `gif_files` (each GIF once, by a
//! hash of where it came from), `saved_gifs` (each account's) and
//! `gif_days` (the provider calls made each day), with the files in the
//! media store like uploads.

use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use super::{Setup, Token};
use crate::app::App;
use crate::db::{self, is_unique_violation, query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{now_ms, timestamp};
use crate::node::NodeDb;
use crate::pb;

/// Who owns the provider's GIFs once stored: nobody, so no account's
/// deletion takes them out of everyone's messages.
pub const OWNER: &str = "gifs";

/// The most GIFs one account saves.
pub const MAX_SAVED: i64 = 200;

/// The biggest GIF fetched from a provider when no cap is set.
const MAX_FETCH: i64 = 32 * 1024 * 1024;

/// GIFs fetched from providers at once.
static FETCHES: Semaphore = Semaphore::const_new(4);

const DAY_MS: i64 = 86_400_000;

/// A sendable GIF, as `gif_files` keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct GifFile {
    pub media_id: String,
    pub provider: i32,
    pub width: i32,
    pub height: i32,
    pub title: String,
}

impl GifFile {
    pub fn sealed(&self, app: &App) -> pb::MessageGif {
        super::sealed(app, &self.media_id, self.width, self.height, &self.title, self.provider)
    }
}

fn key_of(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    super::hex(&hash.finalize())
}

const FILE_COLUMNS: &str = "g.media_id, g.provider, g.width, g.height, g.title";

fn file_row(r: &turso::Row) -> turso::Result<GifFile> {
    Ok(GifFile { media_id: r.get(0)?, provider: r.get(1)?, width: r.get(2)?, height: r.get(3)?, title: r.get(4)? })
}

/// The stored GIF under `key`, if its file is there.
async fn by_key(node: &NodeDb, key: &str) -> Result<Option<GifFile>> {
    let conn = db::connect(node.db())?;
    query_one(
        &conn,
        &format!(
            "SELECT {FILE_COLUMNS} FROM gif_files g JOIN media m ON m.id = g.media_id
             WHERE g.key = ?1 AND m.stored_at IS NOT NULL"
        ),
        [key],
        file_row,
    )
    .await
}

/// The stored GIF whose file is `media_id`.
pub async fn by_media(node: &NodeDb, media_id: &str) -> Result<Option<GifFile>> {
    let conn = db::connect(node.db())?;
    query_one(
        &conn,
        &format!(
            "SELECT {FILE_COLUMNS} FROM gif_files g JOIN media m ON m.id = g.media_id
             WHERE g.media_id = ?1 AND m.stored_at IS NOT NULL"
        ),
        [media_id],
        file_row,
    )
    .await
}

/// Counts one call to the provider today, refusing past `cap`.
pub async fn count_call(node: &NodeDb, now: i64, cap: i64) -> Result<()> {
    let day = now / DAY_MS;
    db::write(node.db(), async |conn| {
        conn.execute(
            "INSERT INTO gif_days (day, calls) VALUES (?1, 1) ON CONFLICT (day) DO UPDATE SET calls = calls + 1",
            [day],
        )
        .await?;
        let calls = query_one(conn, "SELECT calls FROM gif_days WHERE day = ?1", [day], |r| r.get::<i64>(0))
            .await?
            .unwrap_or(0);
        if calls > cap {
            return Err(Error::ResourceExhausted(
                "this instance's GIF searches ran out for today; try again tomorrow".into(),
            ));
        }
        // Old days don't matter.
        conn.execute("DELETE FROM gif_days WHERE day < ?1", [day - 2]).await?;
        Ok(())
    })
    .await
}

/// Stores a search result's GIF (once for the whole instance) and gives it
/// back ready to send.
pub async fn store_found(app: &App, setup: &Setup, token: &Token) -> Result<pb::MessageGif> {
    let node = app.node()?;
    let key = key_of(&["provider", token.provider.report_id(), &token.id]);
    if let Some(file) = by_key(node, &key).await? {
        return Ok(file.sealed(app));
    }
    let limit = setup.gif_bytes.unwrap_or(MAX_FETCH).min(MAX_FETCH);
    let Some(chosen) = token.full.iter().find(|r| r.size == 0 || r.size as i64 <= limit) else {
        return Err(Error::ResourceExhausted(format!(
            "that GIF is bigger than this instance takes ({}); pick another",
            crate::media::size_label(limit)
        )));
    };
    let Ok(_turn) = FETCHES.acquire().await else { return Err(Error::Busy) };
    // Someone may have stored it while this waited.
    if let Some(file) = by_key(node, &key).await? {
        return Ok(file.sealed(app));
    }
    let bytes = download(app, &chosen.url, limit as usize).await.map_err(|reason| {
        crate::reports::server_error("gif_store_failed", Some(setup.provider.report_id()));
        Error::Unavailable(format!("couldn't get that GIF from the provider ({reason}); try another"))
    })?;
    let (width, height) = crate::media::still::gif_size(&bytes).unwrap_or((chosen.width.max(1), chosen.height.max(1)));
    let media_id = crate::media::new_id();
    let size = write_file(app, &media_id, &bytes).await?;
    let file = GifFile {
        media_id: media_id.clone(),
        provider: token.provider.to_pb() as i32,
        width: width as i32,
        height: height as i32,
        title: token.title.clone(),
    };
    match insert(node, &key, &file, size, OWNER).await {
        Ok(()) => Ok(file.sealed(app)),
        Err(err) if is_unique_violation(&err) => {
            // Someone else stored it first: theirs stays.
            app.media()?.remove(&media_id);
            by_key(node, &key).await?.map(|f| f.sealed(app)).ok_or_else(|| Error::internal("a stored GIF went missing"))
        }
        Err(err) => {
            app.media()?.remove(&media_id);
            Err(err)
        }
    }
}

/// The bytes of a provider's GIF: only a GIF, at most `max` bytes.
async fn download(app: &App, url: &str, max: usize) -> std::result::Result<bytes::Bytes, &'static str> {
    let picture = if app.config.gif_api_url.is_some() {
        // Tests: the provider is on this machine; only its own files, capped.
        let base = app.config.gif_api_url.as_deref().unwrap_or_default();
        if !url.starts_with(&format!("{base}/")) {
            return Err("not the test provider");
        }
        let mut response = super::TEST_CLIENT.get(url).send().await.map_err(|_| "it didn't answer")?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "cut off")? {
            if body.len() + chunk.len() > max {
                return Err("too big");
            }
            body.extend_from_slice(&chunk);
        }
        let bytes = bytes::Bytes::from(body);
        crate::outside::Picture {
            content_type: crate::media::sniff(&bytes[..bytes.len().min(16)]).ok_or("not a picture")?,
            bytes,
        }
    } else {
        tokio::time::timeout(std::time::Duration::from_secs(30), crate::outside::fetch_up_to(url, max))
            .await
            .map_err(|_| "too slow")??
    };
    if picture.content_type != "image/gif" {
        return Err("not a GIF");
    }
    Ok(picture.bytes)
}

/// Writes a GIF into the media store without its metadata; its size as kept.
async fn write_file(app: &App, media_id: &str, bytes: &[u8]) -> Result<i64> {
    let media = app.media()?;
    let temp = media.incoming(media_id);
    let kept = async {
        tokio::fs::write(&temp, bytes).await?;
        let size = crate::media::without_metadata(&temp, "image/gif").await?.unwrap_or(bytes.len() as i64);
        tokio::fs::rename(&temp, media.path(media_id)).await?;
        Ok::<_, std::io::Error>(size)
    }
    .await;
    match kept {
        Ok(size) => Ok(size),
        Err(_) => {
            let _ = tokio::fs::remove_file(&temp).await;
            tracing::info!("couldn't keep a GIF from the provider");
            Err(Error::invalid("that GIF looks broken; pick another"))
        }
    }
}

/// The media row (stored and in use) and the `gif_files` row for a new file.
async fn insert(node: &NodeDb, key: &str, file: &GifFile, size: i64, owner: &str) -> Result<()> {
    let now = now_ms();
    db::write(node.db(), async |conn| {
        conn.execute(
            "INSERT INTO media (id, account_id, purpose, content_type, size, created_at, expires_at, stored_at, used_at)
             VALUES (?1, ?2, ?3, 'image/gif', ?4, ?5, ?5, ?5, ?5)",
            (file.media_id.as_str(), owner, pb::MediaPurpose::Gif as i64, size, now),
        )
        .await?;
        conn.execute(
            "INSERT INTO gif_files (key, media_id, provider, width, height, title, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (key, file.media_id.as_str(), file.provider, file.width, file.height, file.title.as_str(), now),
        )
        .await?;
        Ok(())
    })
    .await
}

/// A GIF `account_id` uploaded (`MEDIA_PURPOSE_GIF`), made sendable.
pub async fn from_upload(app: &App, account_id: &str, url: &str) -> Result<GifFile> {
    let Some(upload) = app.check_upload(account_id, pb::MediaPurpose::Gif, url, None).await? else {
        return Err(Error::invalid("upload the GIF to this instance first"));
    };
    let node = app.node()?;
    if let Some(file) = by_media(node, &upload.id).await? {
        return Ok(file);
    }
    if upload.content_type != "image/gif" {
        return Err(Error::invalid("only GIFs can be sent as GIFs"));
    }
    let mut head = [0u8; 10];
    {
        use tokio::io::AsyncReadExt;
        let mut f = tokio::fs::File::open(app.media()?.path(&upload.id)).await?;
        f.read_exact(&mut head).await?;
    }
    let (width, height) =
        crate::media::still::gif_size(&head).ok_or_else(|| Error::invalid("that GIF looks broken"))?;
    let file = GifFile {
        media_id: upload.id.clone(),
        provider: pb::GifProvider::Unspecified as i32,
        width: width as i32,
        height: height as i32,
        title: String::new(),
    };
    let key = key_of(&["upload", &upload.id]);
    let now = now_ms();
    db::write(node.db(), async |conn| {
        conn.execute(
            "INSERT INTO gif_files (key, media_id, provider, width, height, title, created_at)
             VALUES (?1, ?2, 0, ?3, ?4, '', ?5) ON CONFLICT (key) DO NOTHING",
            (key.as_str(), file.media_id.as_str(), file.width, file.height, now),
        )
        .await?;
        Ok(())
    })
    .await?;
    node.use_media(&upload.id, None).await?;
    Ok(file)
}

/// An account's saved GIFs, newest first, and whether each is its own upload.
pub async fn saved(node: &NodeDb, account_id: &str) -> Result<Vec<(GifFile, i64, bool)>> {
    let conn = db::connect(node.db())?;
    query_all(
        &conn,
        &format!(
            "SELECT {FILE_COLUMNS}, s.saved_at, m.account_id = s.account_id
             FROM saved_gifs s JOIN gif_files g ON g.media_id = s.media_id JOIN media m ON m.id = s.media_id
             WHERE s.account_id = ?1 AND m.stored_at IS NOT NULL
             ORDER BY s.saved_at DESC, s.media_id DESC LIMIT ?2"
        ),
        (account_id, MAX_SAVED),
        |r| Ok((file_row(r)?, r.get::<i64>(5)?, r.get::<i64>(6)? != 0)),
    )
    .await
}

/// Saves a GIF for `account_id` (again: to the top), up to [`MAX_SAVED`].
pub async fn save(node: &NodeDb, account_id: &str, media_id: &str) -> Result<i64> {
    let now = now_ms();
    db::write(node.db(), async |conn| {
        let count =
            query_one(conn, "SELECT count(*) FROM saved_gifs WHERE account_id = ?1", [account_id], |r| r.get::<i64>(0))
                .await?
                .unwrap_or(0);
        let there = query_one(
            conn,
            "SELECT 1 FROM saved_gifs WHERE account_id = ?1 AND media_id = ?2",
            (account_id, media_id),
            |r| r.get::<i64>(0),
        )
        .await?
        .is_some();
        if !there && count >= MAX_SAVED {
            return Err(Error::ResourceExhausted(format!("you can save {MAX_SAVED} GIFs; remove one first")));
        }
        conn.execute(
            "INSERT INTO saved_gifs (account_id, media_id, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (account_id, media_id) DO UPDATE SET saved_at = excluded.saved_at",
            (account_id, media_id, now),
        )
        .await?;
        Ok(now)
    })
    .await
}

pub async fn unsave(node: &NodeDb, account_id: &str, media_id: &str) -> Result<()> {
    db::write(node.db(), async |conn| {
        conn.execute("DELETE FROM saved_gifs WHERE account_id = ?1 AND media_id = ?2", (account_id, media_id)).await?;
        Ok(())
    })
    .await
}

/// A saved GIF as apps see it.
pub fn saved_pb(app: &App, file: &GifFile, saved_at: i64, uploaded: bool) -> pb::SavedGif {
    pb::SavedGif { gif: Some(file.sealed(app)), saved_at: Some(timestamp(saved_at)), uploaded }
}
