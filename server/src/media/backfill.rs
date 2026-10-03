//! Pictures kept before metadata was taken out of every upload (and GIFs
//! and AVIFs, until it was taken out of those too) may still say where they
//! were taken. Once per data folder, after start-up, this goes over the
//! pictures kept here and takes it out of each, in place, the way an upload
//! is: accounts' pictures where accounts are kept, servers' pictures on the
//! shard holding them. A cleaned picture goes to the replica again, since
//! the replica only copies files it hasn't got. It runs in the background,
//! one picture at a time, so nobody waits on it; a folder whose pass didn't
//! finish (a restart, an error) is gone over again on the next start, which
//! leaves pictures already clean as they are. How many it cleaned is an
//! anonymous count, nothing more.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app::{App, Link};

/// Written in the data folder once a pass has gone over every picture. A
/// later version that takes out more bumps the number.
const MARKER: &str = ".pictures-cleaned-1";

/// Starts the pass in the background, unless this data folder had it.
pub fn spawn(app: Arc<App>) {
    let marker = app.config.data_path.join(MARKER);
    if marker.exists() {
        return;
    }
    tokio::spawn(async move {
        tokio::select! {
            _ = app.shutdown.cancelled() => {}
            done = run(&app) => match done {
                Ok(cleaned) => {
                    if cleaned > 0 {
                        tracing::info!(cleaned, "took the metadata out of pictures kept from before");
                        crate::reports::server_used("pictures_cleaned", cleaned as u64);
                    }
                    if let Err(err) = std::fs::write(&marker, b"") {
                        tracing::warn!(error = %err, "couldn't note that the pictures were cleaned");
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "couldn't clean every picture kept from before; trying again next start");
                    crate::reports::server_error("pictures_clean", Some("media::backfill"));
                }
            },
        }
    });
}

/// Goes over every picture kept here; says how many it cleaned.
async fn run(app: &App) -> crate::error::Result<usize> {
    let mut cleaned = 0;
    if let (Ok(node), Ok(media)) = (app.node(), app.media()) {
        for (id, content_type) in node.stored_media().await? {
            let path = media.path(&id);
            let Some(size) = clean(&path, &content_type).await? else { continue };
            node.set_media_size(&id, size).await?;
            if let Some(replica) = &app.replica {
                replica.store().put_file(&format!("media/{id}"), &path).await?;
            }
            cleaned += 1;
        }
    }
    if let Link::Shard(_) = &app.link {
        let dir = app.config.data_path.join(crate::cluster::pictures::DIR);
        for server_id in folders(&dir)? {
            // A server moving away takes its pictures as they are.
            if !app.servers.holds(&server_id) || app.servers.frozen().contains(&server_id) {
                continue;
            }
            for (id, path) in crate::cluster::pictures::pictures(&app.config.data_path, &server_id)? {
                let Some(content_type) = kind(&path)? else { continue };
                if clean(&path, content_type).await?.is_none() {
                    continue;
                }
                if let Some(replica) = app.servers.replica() {
                    replica.store().put_file(&crate::cluster::pictures::name(&server_id, &id), &path).await?;
                }
                cleaned += 1;
            }
        }
    }
    Ok(cleaned)
}

/// The folders in `dir`, by name; none when it isn't there.
fn folders(dir: &Path) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Ok(name) = entry.file_name().into_string()
        {
            found.push(name);
        }
    }
    found.sort();
    Ok(found)
}

/// What the picture at `path` is, from its first bytes.
fn kind(path: &Path) -> io::Result<Option<&'static str>> {
    let mut head = Vec::with_capacity(16);
    std::fs::File::open(path)?.take(16).read_to_end(&mut head)?;
    Ok(super::sniff(&head))
}

/// Takes the metadata out of the picture at `path`, in place: its new size
/// when anything came out, `None` when it was already clean (or is gone, or
/// is of a type kept as it came). One whose metadata can't be found is left
/// as it is and skipped, as it's served already.
async fn clean(path: &Path, content_type: &str) -> io::Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    // Beside it, under a name no picture has; a pass cut short leaves it
    // to the next one, which writes over it.
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let copy = path.with_file_name(format!(".cleaning-{name}"));
    tokio::fs::copy(path, &copy).await?;
    let cleaned = match super::without_metadata(&copy, content_type).await {
        Ok(Some(size)) => size,
        Ok(None) | Err(_) => {
            let _ = tokio::fs::remove_file(&copy).await;
            return Ok(None);
        }
    };
    let (original, copy_path) = (path.to_path_buf(), copy.clone());
    let same =
        tokio::task::spawn_blocking(move || same_bytes(&original, &copy_path)).await.map_err(io::Error::other)?;
    if same? || !path.exists() {
        let _ = tokio::fs::remove_file(&copy).await;
        return Ok(None);
    }
    tokio::fs::rename(&copy, path).await?;
    Ok(Some(cleaned))
}

/// Whether two files hold the same bytes.
fn same_bytes(a: &PathBuf, b: &PathBuf) -> io::Result<bool> {
    if std::fs::metadata(a)?.len() != std::fs::metadata(b)?.len() {
        return Ok(false);
    }
    let (mut a, mut b) = (io::BufReader::new(std::fs::File::open(a)?), io::BufReader::new(std::fs::File::open(b)?));
    let (mut x, mut y) = ([0u8; 8192], [0u8; 8192]);
    loop {
        let n = a.read(&mut x)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut y[..n])?;
        if x[..n] != y[..n] {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pictures_are_cleaned_once_and_clean_ones_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let chunk =
            |kind: &[u8; 4], data: &[u8]| [&(data.len() as u32).to_be_bytes()[..], kind, data, &[0, 0, 0, 0]].concat();
        let ihdr = chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        let clean_png = [&b"\x89PNG\r\n\x1a\n"[..], &ihdr, &chunk(b"IEND", &[])].concat();
        let with_text =
            [&b"\x89PNG\r\n\x1a\n"[..], &ihdr, &chunk(b"tEXt", b"GPS\0here"), &chunk(b"IEND", &[])].concat();

        let path = dir.path().join("picture");
        std::fs::write(&path, &with_text).unwrap();
        assert_eq!(kind(&path).unwrap(), Some("image/png"));
        assert_eq!(clean(&path, "image/png").await.unwrap(), Some(clean_png.len() as i64));
        assert_eq!(std::fs::read(&path).unwrap(), clean_png);
        assert_eq!(clean(&path, "image/png").await.unwrap(), None, "already clean");

        // A broken one stays as it is, and a gone one is skipped.
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nbroken").unwrap();
        assert_eq!(clean(&path, "image/png").await.unwrap(), None);
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG\r\n\x1a\nbroken");
        assert_eq!(clean(&dir.path().join("gone"), "image/png").await.unwrap(), None);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "nothing left behind");
    }
}
