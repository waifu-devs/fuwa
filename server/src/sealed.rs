//! Sealed files: what a direct message carries that's too big to go in it,
//! such as a voice message. The device encrypts the file under a key of its
//! own (AES-256-GCM, a new key per file) and uploads the ciphertext; the key
//! travels only inside the end-to-end encrypted message. The instance keeps
//! the bytes with the other uploads (node.db's `media`, purpose `SEALED`),
//! serves them to anyone with the link like a picture, and never opens them.
//!
//! An upload is reserved with `DirectMessageService.CreateSealedUpload` and
//! sent to the same `/media/upload/<token>` as pictures. Naming it in
//! `PostMessage`'s `media_ids` keeps it with that record (dms.db's
//! `record_media`); deleting the record deletes it. One that's never sent
//! is swept after a day, like an unused picture.

use std::path::Path;

use axum::body::Body;
use futures::StreamExt;
use http::StatusCode;
use tokio::io::AsyncWriteExt;

use crate::app::App;
use crate::error::{Error, Result};
use crate::media;
use crate::pb;

/// What sealed files are served as: bytes nobody but the devices can read.
pub const CONTENT_TYPE: &str = "application/octet-stream";

/// The fewest bytes a sealed file can be: its nonce and tag.
pub const MIN_BYTES: i64 = 12 + 16;

/// The most sealed files one message carries.
pub const MAX_PER_MESSAGE: usize = 10;

/// Checks that `ids` are sealed uploads of `account_id`'s, stored and not yet
/// carried by a message, and keeps them from the sweep. Answers their ids in
/// canonical form.
pub async fn carry(app: &App, account_id: &str, ids: &[String]) -> Result<Vec<String>> {
    if ids.len() > MAX_PER_MESSAGE {
        return Err(Error::invalid(format!("a message can carry at most {MAX_PER_MESSAGE} files")));
    }
    let node = app.node()?;
    let dms = app.dms()?;
    let mut carried: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(id) = media::parse_id(id) else {
            return Err(Error::invalid("that isn't a sealed file here"));
        };
        if carried.contains(&id) {
            continue;
        }
        match node.media(&id).await? {
            Some(row) if row.account_id == account_id && row.purpose == pb::MediaPurpose::Sealed && row.stored => {
                if dms.carries(&id).await? {
                    return Err(Error::invalid("that file is already in another message"));
                }
            }
            Some(row) if row.account_id == account_id && row.purpose == pb::MediaPurpose::Sealed => {
                return Err(Error::FailedPrecondition("finish uploading the file first".into()));
            }
            _ => return Err(Error::NotFound("sealed file")),
        }
        carried.push(id);
    }
    // Kept before the record is added: a record that then fails to go in
    // leaves the file kept until the message is sent again (it's retried with
    // the same file) rather than a message whose file was swept.
    for id in &carried {
        node.use_media(id, None).await?;
    }
    Ok(carried)
}

/// Writes a sealed upload's body to `temp`: exactly `size` bytes of anything.
/// Returns the size kept.
pub async fn receive_file(
    id: &str,
    size: i64,
    temp: &Path,
    body: Body,
) -> std::result::Result<i64, (StatusCode, String)> {
    let broken = |_: std::io::Error| {
        tracing::error!(media = %id, "couldn't store a sealed upload");
        (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server".to_string())
    };
    let mut file = tokio::fs::File::create(temp).await.map_err(broken)?;
    let mut stream = body.into_data_stream();
    let mut received: i64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| (StatusCode::BAD_REQUEST, "the upload was cut off".to_string()))?;
        received += chunk.len() as i64;
        if received > size {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("the file is bigger than the {} it was said to be", media::size_label(size)),
            ));
        }
        file.write_all(&chunk).await.map_err(broken)?;
    }
    if received != size {
        return Err((StatusCode::BAD_REQUEST, format!("got {received} bytes of a {size}-byte file")));
    }
    file.sync_all().await.map_err(broken)?;
    Ok(received)
}
