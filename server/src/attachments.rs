//! Files attached to messages: uploads made for a server
//! (`MEDIA_PURPOSE_ATTACHMENT`, see [`crate::media`]) that a message there
//! links to.
//!
//! The server's file keeps a row for each one in `attachments`, written in
//! the same write as its message and deleted with it (or with its channel,
//! or a ban's purge). The row is what serves the file: its name and kind,
//! and only while it's there. The bytes live with the server's pictures:
//! under `media/` in one process, on the shard holding the server otherwise
//! (uploaded straight there, moved and backed up with the server). Once a
//! message lets go of a file, [`drop_soon`] deletes it and its upload's row
//! right after the write; whatever that misses, the hourly sweeps catch.

use std::sync::Arc;

use crate::app::{App, Link};
use crate::db::{query_all, query_one};
use crate::error::Result;
use crate::media::MediaRow;
use crate::pb;
use crate::servers::{self as store, UsageChange};

/// The longest a file's name may be, in characters.
pub const MAX_NAME: usize = 255;

/// A file a message has, as its server keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    pub media_id: String,
    pub filename: String,
    pub content_type: String,
    pub size: i64,
}

impl Attached {
    /// One known only by its upload, named by its id.
    pub fn bare(row: &MediaRow) -> Self {
        Self {
            media_id: row.id.clone(),
            filename: row.id.clone(),
            content_type: row.content_type.clone(),
            size: row.size,
        }
    }
}

/// A file's name as people gave it, made safe to show and to download as:
/// only its last part (no folders), without control characters or the
/// invisible marks that turn text around (which can make `exe.pdf` of
/// `fdp.exe`), at most [`MAX_NAME`] characters; "file" when nothing's left.
pub fn clean_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let turns = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let kept: String = last.chars().filter(|c| !c.is_control() && !turns(*c)).take(MAX_NAME).collect();
    let kept = kept.trim().trim_start_matches('.').trim().to_string();
    if kept.is_empty() { "file".to_string() } else { kept }
}

/// The file a server's message has under `media_id`, if one does.
pub async fn find(app: &App, server_id: &str, media_id: &str) -> Result<Option<Attached>> {
    let sdb = app.servers.get(server_id).await?;
    lookup(&sdb.read()?, media_id).await
}

/// [`find`], on a server file already open.
pub async fn lookup(conn: &turso::Connection, media_id: &str) -> Result<Option<Attached>> {
    query_one(
        conn,
        "SELECT media_id, filename, content_type, size FROM attachments WHERE media_id = ?1",
        [media_id],
        row,
    )
    .await
}

fn row(r: &turso::Row) -> turso::Result<Attached> {
    Ok(Attached { media_id: r.get(0)?, filename: r.get(1)?, content_type: r.get(2)?, size: r.get(3)? })
}

/// Notes the files a new message has, inside the write that stores it, and
/// counts their bytes. A file already in another message clashes on its id,
/// so one file is never in two.
pub async fn add(conn: &turso::Connection, message: &pb::Message, now: i64) -> Result<()> {
    let mut bytes = 0;
    for file in message.attachments.iter().filter(|a| crate::media::parse_id(&a.id).is_some()) {
        conn.execute(
            "INSERT INTO attachments (media_id, message_id, channel_id, filename, content_type, size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                file.id.as_str(),
                message.id.as_str(),
                message.channel_id.as_str(),
                file.filename.as_str(),
                file.content_type.as_str(),
                file.size,
                now,
            ),
        )
        .await?;
        bytes += file.size;
    }
    if bytes > 0 {
        store::add_usage(conn, UsageChange { attachment_bytes: bytes, ..Default::default() }).await?;
    }
    Ok(())
}

/// Lets go of a message's files, inside the write that deletes it, and takes
/// their bytes off the totals. The files themselves go after the write
/// ([`drop_soon`]).
pub async fn forget_message(conn: &turso::Connection, message_id: &str) -> Result<Vec<String>> {
    forget(conn, "message_id", message_id).await
}

/// [`forget_message`] for every message in a channel being deleted.
pub async fn forget_channel(conn: &turso::Connection, channel_id: &str) -> Result<Vec<String>> {
    forget(conn, "channel_id", channel_id).await
}

/// Every file a server's messages have, for when the server is deleted.
pub async fn all(conn: &turso::Connection) -> Result<Vec<String>> {
    query_all(conn, "SELECT media_id FROM attachments", (), |r| r.get::<String>(0)).await
}

async fn forget(conn: &turso::Connection, column: &str, id: &str) -> Result<Vec<String>> {
    let files = query_all(conn, &format!("SELECT media_id, size FROM attachments WHERE {column} = ?1"), [id], |r| {
        Ok((r.get::<String>(0)?, r.get::<i64>(1)?))
    })
    .await?;
    if files.is_empty() {
        return Ok(vec![]);
    }
    conn.execute(&format!("DELETE FROM attachments WHERE {column} = ?1"), [id]).await?;
    let bytes: i64 = files.iter().map(|(_, size)| size).sum();
    store::add_usage(conn, UsageChange { attachment_bytes: -bytes, ..Default::default() }).await?;
    Ok(files.into_iter().map(|(media_id, _)| media_id).collect())
}

/// Deletes files no message has any more, and their uploads' rows, in the
/// background once the write that let go of them is done. One that fails
/// is left to the hourly sweeps: it's no longer served either way.
pub fn drop_soon(app: &Arc<App>, server_id: &str, media_ids: Vec<String>) {
    if media_ids.is_empty() {
        return;
    }
    let (app, server_id) = (app.clone(), server_id.to_string());
    tokio::spawn(async move {
        let base = app.settings().public_url.clone();
        for id in media_ids {
            let link = format!("{base}/media/{id}");
            app.drop_picture(&link, "", crate::api::PictureOwner::Server(&server_id)).await;
        }
    });
}

/// Where a message links to a file uploaded for `server_id`: at the shard
/// holding the server when the instance is split (so it's served without
/// asking the directory), else at the upload's own link.
pub fn link(app: &App, server_id: &str, media_id: &str) -> String {
    let base = app.settings().public_url.clone();
    match &app.link {
        Link::Shard(_) => format!("{base}/media/servers/{server_id}/{media_id}"),
        _ => format!("{base}/media/{media_id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(clean_name("report.pdf"), "report.pdf");
        assert_eq!(clean_name("C:\\Users\\me\\Desktop\\cat.png"), "cat.png");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("fdp\u{202e}exe.pdf"), "fdpexe.pdf");
        assert_eq!(clean_name("line\nbreak\t.txt"), "linebreak.txt");
        assert_eq!(clean_name("  .hidden "), "hidden");
        assert_eq!(clean_name(""), "file");
        assert_eq!(clean_name("folder/"), "file");
        assert_eq!(clean_name(&"あ".repeat(300)).chars().count(), MAX_NAME);
    }
}
