//! Moving a server from one shard to another, and so between regions
//! (docs/regions.md). The directory runs a move:
//!
//! 1. It records the move in node.db (`moves`), so a restart in the middle
//!    finishes or undoes it rather than leaving the server in two places.
//! 2. It asks the new shard to [`adopt`] the server, which takes its files
//!    from the old shard ([`send`]): from then on the old shard takes no
//!    changes to it (the gateway holds them, as it does over a restart),
//!    hangs up its calls, finishes its recordings, folds its log into the
//!    file and sends the file, its recordings and its pictures, each checked
//!    by SHA-256. The new shard opens it and replicates it to its own bucket,
//!    and takes any of its pictures the directory still keeps.
//! 3. It places the server on the new shard (the move is now committed).
//! 4. It tells the old shard to [`release`] it: its files and its replica's
//!    copies go, and live streams following it there end, so gateways follow
//!    it on the new shard from where they were.
//!
//! A move that fails before 3 is undone: the new shard lets go of whatever it
//! took, and the old one takes changes again. Shards learn how moves ended
//! that they missed (the directory restarting, say) when they register.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use tonic::Status;

use super::directory::Shards;
use crate::app::{App, Link};
use crate::cpb;
use crate::error::{Error, Result};
use crate::id::parse_id;
use crate::node::Move;
use crate::pb;

/// How much of a file goes in one message.
const PIECE: usize = 1024 * 1024;
/// Where a moving server's files wait on the new shard until they've all
/// arrived whole.
const INCOMING: &str = "incoming-move";

// ─────────────── The directory ───────────────

/// Moves a server to the emptiest shard up in `region`. Runs to the end even
/// if whoever asked stops waiting.
pub async fn move_server(app: &Arc<App>, server_id: &str, region: &str) -> Result<pb::Server> {
    let app = app.clone();
    let (server_id, region) = (server_id.to_string(), region.to_string());
    tokio::spawn(async move { run(&app, &server_id, &region).await })
        .await
        .map_err(|err| Error::internal(format!("a move stopped: {err}")))?
}

async fn run(app: &Arc<App>, server_id: &str, region: &str) -> Result<pb::Server> {
    let Link::Directory(shards) = &app.link else {
        return Err(Error::FailedPrecondition("this instance keeps every server in one place".into()));
    };
    if !region.is_empty() {
        super::check_region(region).map_err(|err| Error::invalid(format!("region {err}")))?;
    }
    let server_id = parse_id("server_id", server_id)?;
    let from = app.index.placement(&server_id).ok_or(Error::NotFound("server"))?;
    let here = shards.region_of(&from).unwrap_or_default();
    if here == shards.region(region) {
        return Err(Error::FailedPrecondition("that server is already in that region".into()));
    }
    let (to, mut to_client) = shards.emptiest_in(region, &app.index.shard_sizes())?;
    let from_url = shards
        .url_if_up(&from)
        .ok_or_else(|| Error::Unavailable(format!("shard {from}, which holds that server, is down; try again soon")))?;
    let from_client = shards.client(&from)?;

    let mut moved = Move { server_id: server_id.clone(), from: from.clone(), to: to.clone(), committed: false };
    {
        let mut moves = shards.moves.write().unwrap_or_else(|p| p.into_inner());
        if moves.get(&server_id).is_some_and(|(_, running)| *running) {
            return Err(Error::FailedPrecondition("that server is already moving".into()));
        }
        moves.insert(server_id.clone(), (moved.clone(), true));
    }
    let node = app.node()?;
    if let Err(err) = node.save_move(&moved).await {
        shards.moves.write().unwrap_or_else(|p| p.into_inner()).remove(&server_id);
        return Err(err);
    }
    tracing::info!(server = %server_id, %from, %to, region = %shards.region(region), "moving a server");

    let request = cpb::AdoptServerRequest { server_id: server_id.clone(), from_url };
    let entry = match to_client.adopt_server(request).await.map(|r| r.into_inner().entry) {
        Ok(Some(entry)) => entry,
        failed => {
            let err = match failed {
                Err(status) => Error::from(status),
                _ => Error::internal("the new shard didn't say what it took"),
            };
            tracing::warn!(server = %server_id, %from, %to, error = %err, "a move failed; undoing it");
            undo(app, shards, &moved).await;
            return Err(err);
        }
    };

    moved.committed = true;
    if let Err(err) = node.save_move(&moved).await {
        tracing::warn!(server = %server_id, error = %err, "couldn't place a moved server; undoing the move");
        undo(app, shards, &moved).await;
        return Err(err);
    }
    let server = entry.server.clone().ok_or_else(|| Error::internal("the new shard sent no server"))?;
    shards.moves.write().unwrap_or_else(|p| p.into_inner()).insert(server_id.clone(), (moved.clone(), true));
    app.index.insert(server.clone(), entry.member_ids, entry.invite_codes, Some(&to));

    let request = cpb::ReleaseServerRequest { server_id: server_id.clone(), keep: false };
    let released = super::ride_out(app.config.cluster.ride_out, || {
        let (mut client, request) = (from_client.clone(), request.clone());
        async move { client.release_server(request).await }
    })
    .await;
    if let Err(status) = &released {
        // It's told again when it next registers.
        tracing::warn!(server = %server_id, %from, error = %status.message(), "the old shard hasn't let go of a moved server yet");
    }
    // The new shard replicates it from now on.
    let request = cpb::ReleaseServerRequest { server_id: server_id.clone(), keep: true };
    let kept = super::ride_out(app.config.cluster.ride_out, || {
        let (mut client, request) = (to_client.clone(), request.clone());
        async move { client.release_server(request).await }
    })
    .await;
    if let Err(status) = &kept {
        tracing::warn!(server = %server_id, %to, error = %status.message(), "the new shard hasn't started replicating a moved server yet");
    }
    shards.moves.write().unwrap_or_else(|p| p.into_inner()).insert(server_id.clone(), (moved, false));
    if released.is_ok() && kept.is_ok() {
        settled(app, &server_id).await;
    }
    tracing::info!(server = %server_id, %from, %to, "moved a server");
    Ok(server)
}

/// Puts things back after a move that didn't get as far as placing the
/// server on its new shard. Whatever can't be reached now is set right when
/// it registers.
async fn undo(app: &App, shards: &Shards, moved: &Move) {
    let release = |shard: &str, keep: bool| {
        let client = shards.client(shard);
        let request = cpb::ReleaseServerRequest { server_id: moved.server_id.clone(), keep };
        async move {
            match client {
                Ok(mut client) => client.release_server(request).await.is_ok(),
                Err(_) => false,
            }
        }
    };
    let to_done = release(&moved.to, false).await;
    let from_done = release(&moved.from, true).await;
    if to_done && from_done {
        settled(app, &moved.server_id).await;
    } else {
        // Not running any more: each shard hears how it ended when it registers.
        if let Some(entry) = shards.moves.write().unwrap_or_else(|p| p.into_inner()).get_mut(&moved.server_id) {
            entry.1 = false;
        }
    }
}

/// A move is over everywhere.
pub async fn settled(app: &App, server_id: &str) {
    if let Link::Directory(shards) = &app.link {
        let mut moves = shards.moves.write().unwrap_or_else(|p| p.into_inner());
        if moves.get(server_id).is_some_and(|(_, running)| *running) {
            return;
        }
        moves.remove(server_id);
    }
    if let Err(err) = async { app.node()?.end_move(server_id).await }.await {
        tracing::warn!(server = %server_id, error = %err, "couldn't note that a move is over");
    }
}

/// Takes the servers a shard registers that are on their way to or from it
/// out of what it says (so they stay placed where the move has them), and
/// works out what to tell it: servers it has that moved away, and servers
/// still moving off it. Also returns moves that turn out to be over.
pub fn sort_registration(
    shards: &Shards,
    shard: &str,
    entries: &mut Vec<cpb::ServerEntry>,
) -> (cpb::RegisterShardResponse, Vec<String>) {
    let claimed: HashSet<String> =
        entries.iter().filter_map(|entry| entry.server.as_ref().map(|s| s.id.clone())).collect();
    let mut answer = cpb::RegisterShardResponse::default();
    let mut left_out = HashSet::new();
    let mut settled = Vec::new();
    for (id, (moved, running)) in shards.moves.read().unwrap_or_else(|p| p.into_inner()).iter() {
        let has = claimed.contains(id);
        if moved.from == shard {
            if *running {
                answer.moving.push(id.clone());
            }
            match (moved.committed, has) {
                (true, true) => {
                    answer.moved_away.push(id.clone());
                    left_out.insert(id.clone());
                }
                (true, false) if !running => settled.push(id.clone()),
                _ => {}
            }
        } else if moved.to == shard && moved.committed {
            // Replicated once the old shard has let go, as the move says
            // while it runs; after, right away. Over once the old shard has
            // let go (it registers without it).
            if *running {
                answer.moving.push(id.clone());
            }
        } else if moved.to == shard && !moved.committed {
            if *running {
                answer.moving.push(id.clone());
            }
            match has {
                true => {
                    left_out.insert(id.clone());
                    if !running {
                        answer.moved_away.push(id.clone());
                    }
                }
                false if !running => settled.push(id.clone()),
                false => {}
            }
        }
    }
    entries.retain(|entry| entry.server.as_ref().is_none_or(|s| !left_out.contains(&s.id)));
    (answer, settled)
}

// ─────────────── The old shard ───────────────

/// Sends a server's files to the shard taking it, after stopping it taking
/// changes. It stays that way until [`release`].
pub fn send(app: Arc<App>, server_id: String) -> mpsc::Receiver<Result<cpb::SendServerResponse, Status>> {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        let sent = async {
            let sdb = app.servers.get(&server_id).await?;
            let id = sdb.id.clone();
            app.servers.freeze(&id, true);
            crate::api::hang_up_server(&app, &id).await;
            app.recordings.finish_server(&id).await;
            // Nothing writes while its files are read, and the log is folded
            // in, so the file alone is the whole server.
            let _alone = sdb.db().alone().await;
            sdb.db().checkpoint().await?;
            let mut files: Vec<(String, PathBuf)> = Vec::new();
            for path in app.servers.files(&id) {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                files.push((format!("servers/{name}"), path));
            }
            let recordings = recordings_dir(&app.config.data_path, &id);
            for (relative, path) in walk(&recordings)? {
                files.push((format!("recordings/{id}/{relative}"), path));
            }
            super::pictures::restore_all(&app, &id).await?;
            for (picture, path) in super::pictures::pictures(&app.config.data_path, &id)? {
                files.push((super::pictures::name(&id, &picture), path));
            }
            for (name, path) in files {
                if !send_file(&tx, &name, &path).await? {
                    return Ok(());
                }
            }
            Ok::<_, Error>(())
        }
        .await;
        if let Err(err) = sent {
            let _ = tx.send(Err(err.into())).await;
        }
    });
    rx
}

/// Sends one file piece by piece, ending with its SHA-256. False if the
/// other end stopped listening.
async fn send_file(
    tx: &mpsc::Sender<Result<cpb::SendServerResponse, Status>>,
    name: &str,
    path: &Path,
) -> Result<bool> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; PIECE];
    loop {
        let read = file.read(&mut buffer).await?;
        let piece = if read == 0 {
            let sha256 = std::mem::take(&mut hash).finalize().to_vec();
            cpb::SendServerResponse { path: name.to_string(), data: Vec::new(), sha256 }
        } else {
            hash.update(&buffer[..read]);
            cpb::SendServerResponse { path: name.to_string(), data: buffer[..read].to_vec(), sha256: Vec::new() }
        };
        if tx.send(Ok(piece)).await.is_err() {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
    }
}

/// Ends a move for this shard. With `keep` the server carries on here: the
/// old shard takes changes again (the move didn't happen), and the new one
/// starts replicating it (it did), with its recordings. Without, this shard
/// lets go of it, deleting its files and recordings here.
pub async fn release(app: &App, server_id: &str, keep: bool) -> Result<()> {
    let id = parse_id("server_id", server_id)?;
    if keep {
        if app.servers.freeze(&id, false) {
            tracing::info!(server = %id, "a move didn't happen; the server takes changes here again");
        }
        let adopted = app.servers.replicate_adopted(&id).await?;
        if adopted && let Some(replica) = app.servers.replica() {
            let data = &app.config.data_path;
            for (relative, path) in walk(&recordings_dir(data, &id))? {
                replica.store().put_file(&format!("recordings/{id}/{relative}"), &path).await?;
            }
            super::pictures::replicate(app, &id).await?;
        }
        if adopted {
            // Pictures it used before servers' shards kept them, or that
            // couldn't be taken then.
            match super::pictures::take_all(app, &id).await {
                Ok(0) => {}
                Ok(taken) => tracing::info!(server = %id, taken, "took a moved server's pictures from the directory"),
                Err(err) => {
                    tracing::warn!(server = %id, error = %err, "couldn't take a moved server's pictures");
                    crate::reports::server_error("server_picture_take", Some("cluster::moves"));
                }
            }
        }
        return Ok(());
    }
    // Whatever a failed adoption left behind.
    remove_dir(&app.config.data_path.join(INCOMING).join(&id))?;
    if !app.servers.holds(&id) {
        return Ok(());
    }
    app.servers.freeze(&id, true);
    crate::api::hang_up_server(app, &id).await;
    app.recordings.finish_server(&id).await;
    app.servers.release(&id).await?;
    remove_dir(&recordings_dir(&app.config.data_path, &id))?;
    remove_dir(&super::pictures::server_dir(&app.config.data_path, &id))?;
    tracing::info!(server = %id, "let go of a server that moved to another shard");
    Ok(())
}

/// Does what a registration's answer says: lets go of servers that moved
/// away, and takes changes again to any frozen for a move that's over.
pub async fn after_registering(app: &App, answer: &cpb::RegisterShardResponse) {
    for server_id in &answer.moved_away {
        if let Err(err) = release(app, server_id, false).await {
            tracing::warn!(server = %server_id, error = %err, "couldn't let go of a server that moved away");
        }
    }
    for server_id in app.servers.frozen().into_iter().chain(app.servers.unreplicated()) {
        if !answer.moving.contains(&server_id)
            && !answer.moved_away.contains(&server_id)
            && let Err(err) = release(app, &server_id, true).await
        {
            tracing::warn!(server = %server_id, error = %err, "couldn't carry on with a server after a move");
        }
    }
}

// ─────────────── The new shard ───────────────

/// Takes a server from the shard at `from_url`: copies its files and checks
/// them, puts them in place, opens it and replicates it. Anything left over
/// from a try that failed is replaced.
pub async fn adopt(app: &Arc<App>, server_id: &str, from_url: &str) -> Result<cpb::ServerEntry> {
    let id = parse_id("server_id", server_id)?;
    if app.servers.holds(&id) {
        return Err(Error::AlreadyExists(format!("server {id} is already on this shard")));
    }
    let data = &app.config.data_path;
    if app.servers.files(&id).iter().any(|path| path.exists()) {
        return Err(Error::FailedPrecondition(format!(
            "servers/ here already has files for server {id}; move them out of the way first"
        )));
    }
    let incoming = data.join(INCOMING).join(&id);
    remove_dir(&incoming)?;
    std::fs::create_dir_all(&incoming)?;
    let received = async {
        let key = app.config.cluster.key_value()?;
        let mut from = super::shard_client(from_url, key)?;
        let stream = from.send_server(cpb::SendServerRequest { server_id: id.clone() }).await?.into_inner();
        receive(stream, &id, &incoming).await
    }
    .await;
    let names = match received {
        Ok(names) => names,
        Err(err) => {
            let _ = remove_dir(&incoming);
            return Err(err);
        }
    };
    if !names.contains(&format!("servers/{id}.db")) {
        let _ = remove_dir(&incoming);
        return Err(Error::internal(format!("the old shard didn't send server {id}'s database")));
    }
    for name in &names {
        let dest = data.join(name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(incoming.join(name), dest)?;
    }
    remove_dir(&incoming)?;

    // Replicated from here once the old shard has deleted its replica's
    // copies (which may be in the same bucket): see [`release`] with `keep`.
    let opened = async {
        app.servers.adopt(&id).await?;
        app.servers.entry(&id).await
    }
    .await;
    match opened {
        Ok(entry) => {
            tracing::info!(server = %id, files = names.len(), "took over a server from another shard");
            Ok(entry)
        }
        Err(err) => {
            if let Err(cleanup) = release(app, &id, false).await {
                tracing::warn!(server = %id, error = %cleanup, "couldn't tidy up after a failed move");
            }
            for name in &names {
                let _ = std::fs::remove_file(data.join(name));
            }
            Err(err)
        }
    }
}

/// Writes what the old shard sends into `incoming`, checking each file.
/// Returns the files' paths, from the data folder.
async fn receive(
    mut stream: tonic::Streaming<cpb::SendServerResponse>,
    id: &str,
    incoming: &Path,
) -> Result<Vec<String>> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    let mut names = Vec::new();
    let mut file: Option<(String, tokio::fs::File, Sha256)> = None;
    while let Some(piece) = stream.message().await.map_err(Error::retried)? {
        if !moved_path_ok(&piece.path, id) {
            return Err(Error::internal(format!(
                "the old shard sent {:?}, which isn't one of the server's files",
                piece.path
            )));
        }
        let (name, mut out, mut hash) = match file.take() {
            Some((name, out, hash)) if name == piece.path => (name, out, hash),
            Some((name, ..)) => return Err(Error::internal(format!("the old shard stopped partway through {name}"))),
            None => {
                if names.contains(&piece.path) {
                    return Err(Error::internal(format!("the old shard sent {} twice", piece.path)));
                }
                let dest = incoming.join(&piece.path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                (piece.path.clone(), tokio::fs::File::create(dest).await?, Sha256::new())
            }
        };
        out.write_all(&piece.data).await?;
        hash.update(&piece.data);
        if piece.sha256.is_empty() {
            file = Some((name, out, hash));
            continue;
        }
        out.sync_all().await?;
        if hash.finalize().as_slice() != piece.sha256.as_slice() {
            return Err(Error::internal(format!("{name} didn't arrive intact")));
        }
        names.push(name);
    }
    if let Some((name, ..)) = file {
        return Err(Error::internal(format!("the old shard stopped partway through {name}")));
    }
    Ok(names)
}

/// Whether a path the old shard sent is one of this server's files:
/// `servers/<id>.db` (or a file kept beside it), a recording's
/// `recordings/<id>/<recording>/<file>` or a picture's
/// `server-pictures/<id>/<picture>`. Never anything else, or anywhere else.
fn moved_path_ok(path: &str, id: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["servers", name] => super::is_server_file(name) && name.starts_with(&format!("{id}.db")),
        ["recordings", server, recording, file] => {
            *server == id
                && parse_id("recording", recording).is_ok_and(|parsed| parsed == *recording)
                && !file.is_empty()
                && !file.starts_with('.')
                && file.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        }
        [super::pictures::DIR, server, picture] => {
            *server == id && crate::media::parse_id(picture).is_some_and(|parsed| parsed == *picture)
        }
        _ => false,
    }
}

fn recordings_dir(data: &Path, server_id: &str) -> PathBuf {
    data.join("recordings").join(server_id)
}

/// Every file under `dir` (none if it doesn't exist), by path relative to it.
fn walk(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let mut pending = vec![(String::new(), dir.to_path_buf())];
    while let Some((prefix, dir)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else { continue };
            let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push((relative, entry.path()));
            } else if kind.is_file() {
                found.push((relative, entry.path()));
            }
        }
    }
    found.sort();
    Ok(found)
}

fn remove_dir(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_servers_own_files_move() {
        let id = "01J9Z3K8X2V5W7Q4R6T8Y0B2C4";
        let rec = "01J9Z3K8X2V5W7Q4R6T8Y0B2C5";
        assert!(moved_path_ok(&format!("servers/{id}.db"), id));
        assert!(moved_path_ok(&format!("servers/{id}.db-log"), id));
        assert!(moved_path_ok(&format!("recordings/{id}/{rec}/{rec}.opus.sealed"), id));
        let picture = crate::media::new_id();
        assert!(moved_path_ok(&format!("server-pictures/{id}/{picture}"), id));
        for bad in [
            format!("servers/{rec}.db"),
            "servers/node.db".to_string(),
            format!("recordings/{rec}/{rec}/a.opus"),
            format!("recordings/{id}/../a.opus"),
            format!("recordings/{id}/{rec}/../x"),
            format!("recordings/{id}/{rec}/.hidden"),
            format!("recordings/{id}/{rec}/a/b"),
            format!("../servers/{id}.db"),
            format!("/servers/{id}.db"),
            "node.db".to_string(),
            format!("server-pictures/{rec}/{picture}"),
            format!("server-pictures/{id}/.incoming-{picture}"),
            format!("server-pictures/{id}/{}", picture.to_uppercase()),
            format!("server-pictures/{id}/{picture}/x"),
            format!("media/{picture}"),
        ] {
            assert!(!moved_path_ok(&bad, id), "{bad}");
        }
    }

    #[test]
    fn walks_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/c.opus"), "x").unwrap();
        std::fs::write(dir.path().join("a/d.opus"), "y").unwrap();
        let found: Vec<String> = walk(dir.path()).unwrap().into_iter().map(|(name, _)| name).collect();
        assert_eq!(found, ["a/b/c.opus", "a/d.opus"]);
        assert!(walk(&dir.path().join("missing")).unwrap().is_empty());
    }
}
