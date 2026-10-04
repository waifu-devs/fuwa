use std::collections::HashSet;
use std::sync::atomic::Ordering;

use super::*;

const MIGRATIONS: &[&str] = &["CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL);"];

fn key() -> EncryptionKey {
    EncryptionKey::parse(&"5a".repeat(32)).unwrap()
}

async fn open(path: &Path) -> Arc<Db> {
    Arc::new(db::open(path, Some(&key()), MIGRATIONS).await.unwrap())
}

async fn insert(db: &Db, from: i64, count: i64, pad: usize) {
    for id in from..from + count {
        let v = format!("{id}:{}", "x".repeat(pad));
        db::write(db, async |conn| {
            conn.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", (id, v.as_str())).await?;
            Ok(())
        })
        .await
        .unwrap();
    }
}

async fn rows(path: &Path) -> i64 {
    let db = db::open(path, Some(&key()), &[]).await.unwrap();
    db::query_one(&db::connect(&db).unwrap(), "SELECT count(*) FROM t", (), |r| r.get::<i64>(0)).await.unwrap().unwrap()
}

/// A replica writing into a folder, folding logs at `checkpoint` bytes.
fn replica(data: &Path, store: &Path, checkpoint: u64, rebase: u64) -> Arc<Replica> {
    Replica::with_sizes(Store::dir(store), Duration::from_secs(1), data, None, checkpoint, rebase).unwrap()
}

/// A shard's replica.
fn shard_replica(data: &Path, store: &Path, shard: &str) -> Arc<Replica> {
    Replica::with_sizes(
        Store::dir(store),
        Duration::from_secs(1),
        data,
        Some(shard.into()),
        CHECKPOINT_BYTES,
        REBASE_BYTES,
    )
    .unwrap()
}

async fn restored(store: &Store, name: &str, dir: &Path) -> (i64, bool) {
    let dest = dir.join(format!("{}.db", new_id()));
    let whole = restore_file(store, name, &dest, Some(&key())).await.unwrap();
    (rows(&dest).await, whole)
}

async fn generations(store: &Store, name: &str) -> BTreeSet<String> {
    store
        .list(&format!("{name}/"))
        .await
        .unwrap()
        .into_iter()
        .filter_map(|o| o.key.strip_suffix("/snapshot").and_then(|k| k.rsplit('/').next()).map(String::from))
        .collect()
}

async fn generation_in_use(store: &Store, name: &str) -> String {
    current(store, name).await.unwrap().unwrap().generation
}

#[tokio::test]
async fn ships_every_commit_and_restores_them() {
    let dir = tempfile::tempdir().unwrap();
    let (data, bucket, out) = (dir.path().join("data"), dir.path().join("bucket"), dir.path().join("out"));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let db = open(&data.join("a.db")).await;
    let replica = replica(&data, &bucket, CHECKPOINT_BYTES, REBASE_BYTES);
    replica.track("servers/a", db.clone()).await.unwrap();

    insert(&db, 0, 50, 10).await;
    replica.sync_files().await;
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (50, true));

    insert(&db, 50, 25, 10).await;
    replica.sync_files().await;
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (75, true));

    // Only whole bytes of the log went up, and only encrypted ones: no row's text is readable.
    for object in replica.store().list("servers/a/").await.unwrap() {
        let bytes = replica.store().get(&object.key).await.unwrap().unwrap();
        assert!(!bytes.windows(4).any(|w| w == b"xxxx"), "{} holds plain text", object.key);
    }
    // The wrong key can't restore it.
    let wrong = EncryptionKey::parse(&"a5".repeat(32)).unwrap();
    assert!(restore_file(replica.store(), "servers/a", &out.join("wrong.db"), Some(&wrong)).await.is_err());
}

#[tokio::test]
async fn folds_the_log_and_restores_across_epochs() {
    let dir = tempfile::tempdir().unwrap();
    let (data, bucket, out) = (dir.path().join("data"), dir.path().join("bucket"), dir.path().join("out"));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let db = open(&data.join("a.db")).await;
    let replica = replica(&data, &bucket, 20_000, REBASE_BYTES);
    replica.track("servers/a", db.clone()).await.unwrap();

    for round in 0..12 {
        insert(&db, round * 10, 10, 1500).await;
        replica.sync_files().await;
        // Turso no longer folds the log by itself; the replica keeps it small.
        assert!(size(&db.log_path()) < 30_000, "the log grew to {}", size(&db.log_path()));
    }
    let objects = replica.store().list("servers/a/").await.unwrap();
    let epochs: BTreeSet<u32> =
        objects.iter().filter_map(|o| o.key.rsplit('/').next().and_then(parse_segment)).map(|(e, _)| e).collect();
    assert!(epochs.len() >= 4, "only epochs {epochs:?}");
    assert_eq!(generations(replica.store(), "servers/a").await.len(), 1);
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (120, true));

    // A segment gone missing: restored up to there, and said so.
    let lost = objects.iter().find(|o| o.key.ends_with(&format!("/{:08x}-0000000000000000", 2))).unwrap();
    replica.store().delete(&lost.key).await.unwrap();
    let (count, whole) = restored(replica.store(), "servers/a", &out).await;
    assert!(!whole);
    assert!(count > 0 && count < 120, "{count}");
}

#[tokio::test]
async fn starts_a_new_generation_once_the_logs_outgrow_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (data, bucket, out) = (dir.path().join("data"), dir.path().join("bucket"), dir.path().join("out"));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let db = open(&data.join("a.db")).await;
    let replica = replica(&data, &bucket, 16_000, 40_000);
    replica.track("servers/a", db.clone()).await.unwrap();

    let mut seen = BTreeSet::new();
    for round in 0..20 {
        insert(&db, round * 10, 10, 1500).await;
        replica.sync_files().await;
        seen.insert(generation_in_use(replica.store(), "servers/a").await);
        // The one in use and the one before it, no more.
        assert!(generations(replica.store(), "servers/a").await.len() <= 2);
    }
    assert!(seen.len() >= 3, "only {} generations", seen.len());
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (200, true));
}

#[tokio::test]
async fn a_restart_carries_on_where_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let (data, bucket, out) = (dir.path().join("data"), dir.path().join("bucket"), dir.path().join("out"));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let path = data.join("a.db");
    let first = {
        let db = open(&path).await;
        let replica = replica(&data, &bucket, CHECKPOINT_BYTES, REBASE_BYTES);
        replica.track("servers/a", db.clone()).await.unwrap();
        insert(&db, 0, 30, 10).await;
        replica.sync_files().await;
        insert(&db, 30, 5, 10).await;
        // Shutting down ships what's left.
        replica.close().await;
        generation_in_use(replica.store(), "servers/a").await
    };

    let db = open(&path).await;
    let replica = replica(&data, &bucket, CHECKPOINT_BYTES, REBASE_BYTES);
    replica.track("servers/a", db.clone()).await.unwrap();
    insert(&db, 35, 5, 10).await;
    replica.sync_files().await;
    assert_eq!(generation_in_use(replica.store(), "servers/a").await, first, "a restart started a new generation");
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (40, true));

    // The file folded behind the replica's back: a new generation, and nothing lost.
    {
        let _alone = db.alone().await;
        db.checkpoint().await.unwrap();
    }
    insert(&db, 40, 5, 10).await;
    replica.sync_files().await;
    assert_ne!(generation_in_use(replica.store(), "servers/a").await, first);
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (45, true));
}

#[tokio::test]
async fn a_second_writer_stops_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    let (one, two) = (dir.path().join("one"), dir.path().join("two"));
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    let (db_one, db_two) = (open(&one.join("a.db")).await, open(&two.join("a.db")).await);
    let (first, second) = (
        replica(&one, &bucket, CHECKPOINT_BYTES, REBASE_BYTES),
        replica(&two, &bucket, CHECKPOINT_BYTES, REBASE_BYTES),
    );
    first.fence_every_ms.store(0, Ordering::Relaxed);
    first.track("servers/a", db_one.clone()).await.unwrap();
    second.track("servers/a", db_two.clone()).await.unwrap();

    insert(&db_one, 0, 5, 10).await;
    first.sync_files().await;
    insert(&db_two, 0, 7, 10).await;
    second.sync_files().await;
    let taken = generation_in_use(first.store(), "servers/a").await;
    insert(&db_one, 5, 5, 10).await;
    first.sync_files().await;
    assert_eq!(generation_in_use(first.store(), "servers/a").await, taken);
    let tracked = first.tracked().pop().unwrap();
    assert!(tracked.state.lock().await.fenced);
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    assert_eq!(restored(first.store(), "servers/a", &out).await.0, 7);
}

#[tokio::test]
async fn a_log_the_replica_cant_take_is_still_folded() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let (bucket, away) = (dir.path().join("bucket"), dir.path().join("away"));
    let db = open(&data.join("a.db")).await;
    let first = replica(&data, &bucket, 4_000, REBASE_BYTES);
    first.track("servers/a", db.clone()).await.unwrap();
    insert(&db, 0, 1, 10).await;
    first.sync_files().await;

    // The bucket goes down (its folder turns into a file). The log grows up
    // to its limit, then is folded in anyway.
    std::fs::rename(&bucket, &away).unwrap();
    std::fs::write(&bucket, b"").unwrap();
    insert(&db, 1, 10, 1500).await;
    first.sync_files().await;
    let log = size(&db.log_path());
    assert!(log > 4_000, "{log}");
    insert(&db, 11, 59, 1500).await;
    first.sync_files().await;
    assert!(size(&db.log_path()) < 4_000, "the log was folded in");
    assert!(first.tracked().pop().unwrap().state.lock().await.position.is_none());

    // Once it's back, a new generation has everything.
    std::fs::remove_file(&bucket).unwrap();
    std::fs::rename(&away, &bucket).unwrap();
    insert(&db, 70, 5, 10).await;
    first.sync_files().await;
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    assert_eq!(restored(first.store(), "servers/a", &out).await, (75, true));

    // A file another process took over is still folded as its log fills.
    let other = replica(&dir.path().join("other"), &bucket, REBASE_BYTES, REBASE_BYTES);
    let other_db = open(&dir.path().join("other.db")).await;
    other.track("servers/a", other_db).await.unwrap();
    other.sync_files().await;
    first.fence_every_ms.store(0, Ordering::Relaxed);
    first.sync_files().await;
    assert!(first.tracked().pop().unwrap().state.lock().await.fenced);
    insert(&db, 75, 10, 1500).await;
    first.sync_files().await;
    assert!(size(&db.log_path()) < 4_000, "the log was folded in");
}

#[tokio::test]
async fn each_part_restores_its_own_files_but_not_deleted_servers() {
    let dir = tempfile::tempdir().unwrap();
    let bucket = dir.path().join("bucket");
    let path = |name: &str| {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.join("servers")).unwrap();
        std::fs::create_dir_all(path.join("media")).unwrap();
        path
    };

    // A directory with accounts and a picture, and two shards with servers.
    let directory = path("directory");
    let at_directory = replica(&directory, &bucket, CHECKPOINT_BYTES, REBASE_BYTES);
    let node = open(&directory.join("node.db")).await;
    at_directory.track("node", node.clone()).await.unwrap();
    insert(&node, 0, 2, 10).await;
    let picture = crate::media::new_id();
    std::fs::write(directory.join("media").join(&picture), b"not really a png").unwrap();
    std::fs::write(directory.join("media").join("not-a-picture"), b"skip me").unwrap();
    at_directory.track_media(directory.join("media"));
    at_directory.sync_files().await;
    assert_eq!(at_directory.sync_media().await.unwrap(), 1);
    assert_eq!(at_directory.sync_media().await.unwrap(), 0);

    let (one, two) = (path("one"), path("two"));
    let (at_one, at_two) = (shard_replica(&one, &bucket, "shard-1"), shard_replica(&two, &bucket, "shard-2"));
    let (kept, gone, elsewhere) = (new_id(), new_id(), new_id());
    for (id, data, replica) in [(&kept, &one, &at_one), (&gone, &one, &at_one), (&elsewhere, &two, &at_two)] {
        let db = open(&data.join("servers").join(format!("{id}.db"))).await;
        replica.track(&crate::servers::replica_name(id), db.clone()).await.unwrap();
        insert(&db, 0, 3, 10).await;
        replica.sync_files().await;
    }
    at_one.forget(&crate::servers::replica_name(&gone), true).await;
    assert_eq!(held_by(at_one.store(), "shard-1").await.unwrap(), std::slice::from_ref(&kept));
    assert_eq!(held_by(at_one.store(), "shard-2").await.unwrap(), std::slice::from_ref(&elsewhere));

    // Empty folders over this replica won't start without being told to restore.
    let config =
        ReplicaConfig { target: Target::Dir(bucket.clone()), interval: Duration::from_secs(1), restore: Restore::Off };
    let store = at_directory.store();
    let (new_directory, new_one) = (dir.path().join("new-directory"), dir.path().join("new-one"));
    for (data, part) in [(&new_directory, Part::Directory), (&new_one, Part::Shard("shard-1"))] {
        std::fs::create_dir_all(data).unwrap();
        let refused = prepare(&config, store, data, Some(&key()), part).await.unwrap_err();
        assert!(refused.to_string().contains("FUWA_RESTORE=if-empty"), "{refused}");
    }
    assert!(!new_directory.join("node.db").exists());
    // A shard nobody has seen before starts empty, and so does one whose
    // volume has replicated before (it just holds no servers).
    prepare(&config, store, &dir.path().join("new-three"), Some(&key()), Part::Shard("shard-3")).await.unwrap();
    let been_here = dir.path().join("been-here");
    std::fs::create_dir_all(been_here.join("replica")).unwrap();
    std::fs::write(been_here.join("replica").join("writer"), "someone").unwrap();
    prepare(&config, store, &been_here, Some(&key()), Part::Shard("shard-1")).await.unwrap();
    assert!(!been_here.join("servers").exists());

    // Told to, the directory gets accounts and pictures, each shard its servers.
    let config = ReplicaConfig { restore: Restore::IfEmpty, ..config };
    prepare(&config, store, &new_directory, Some(&key()), Part::Directory).await.unwrap();
    assert_eq!(rows(&new_directory.join("node.db")).await, 2);
    assert_eq!(std::fs::read(new_directory.join("media").join(&picture)).unwrap(), b"not really a png");
    assert!(!new_directory.join("media").join("not-a-picture").exists());
    assert!(!new_directory.join("servers").exists());
    prepare(&config, store, &new_one, Some(&key()), Part::Shard("shard-1")).await.unwrap();
    let servers = |data: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(data.join("servers"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(servers(&new_one), [format!("{kept}.db")], "its own servers, not deleted ones or others'");
    assert_eq!(rows(&new_one.join("servers").join(format!("{kept}.db"))).await, 3);
    // Once its files are there, nothing more happens.
    prepare(&config, store, &new_directory, Some(&key()), Part::Directory).await.unwrap();
    prepare(&config, store, &new_one, Some(&key()), Part::Shard("shard-1")).await.unwrap();

    // A server that moves to another shard goes with it.
    std::fs::copy(new_one.join("servers").join(format!("{kept}.db")), two.join("servers").join(format!("{kept}.db")))
        .unwrap();
    let moved = open(&two.join("servers").join(format!("{kept}.db"))).await;
    assert_eq!(
        db::query_one(&db::connect(&moved).unwrap(), "SELECT count(*) FROM t", (), |r| r.get::<i64>(0)).await.unwrap(),
        Some(3)
    );
    at_two.track(&crate::servers::replica_name(&kept), moved).await.unwrap();
    at_two.sync_files().await;
    let mut held = held_by(store, "shard-2").await.unwrap();
    held.sort();
    let mut expected = vec![kept.clone(), elsewhere.clone()];
    expected.sort();
    assert_eq!(held, expected);
    assert!(held_by(store, "shard-1").await.unwrap().is_empty());

    // Pictures nothing knows are deleted from the replica.
    at_directory.drop_media(&[]).await;
    assert_eq!(at_directory.prune_media(&HashSet::from([picture.clone()])).await.unwrap(), 0);
    assert_eq!(at_directory.prune_media(&HashSet::new()).await.unwrap(), 1);
    assert!(store.list("media/").await.unwrap().is_empty());

    let lines = describe(store).await.unwrap();
    assert!(lines.iter().any(|line| line.starts_with(&format!("servers/{gone}")) && line.ends_with("deleted")));
    assert!(lines.iter().any(|line| line.starts_with(&format!("servers/{kept}")) && line.contains("shard shard-2")));
}

#[tokio::test]
async fn an_empty_replica_lets_a_new_instance_start() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::dir(&dir.path().join("bucket"));
    let config = ReplicaConfig {
        target: Target::Dir(dir.path().join("bucket")),
        interval: Duration::from_secs(1),
        restore: Restore::Off,
    };
    prepare(&config, &store, dir.path(), None, Part::Directory).await.unwrap();
    prepare(&config, &store, dir.path(), None, Part::Shard("shard-1")).await.unwrap();
    assert!(!dir.path().join("node.db").exists());
}

// ─────────────────────── The same, through a fake S3 ───────────────────────

mod fake_s3 {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::{Path, RawQuery, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;

    type Objects = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;

    /// A path-style S3 that keeps objects in memory and lists two at a time,
    /// so paging gets used. It checks every request is signed.
    pub async fn start() -> (String, Objects) {
        let objects: Objects = Arc::default();
        let app = Router::new()
            .route("/bucket", get(list))
            .route("/bucket/{*key}", get(read).put(write).delete(remove))
            .with_state(objects.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), objects)
    }

    fn signed(headers: &HeaderMap) -> Result<(), StatusCode> {
        let authorization = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
        let ok = authorization.starts_with("AWS4-HMAC-SHA256 Credential=id/")
            && authorization.contains("/auto/s3/aws4_request")
            && headers.contains_key("x-amz-date")
            && headers.contains_key("x-amz-content-sha256");
        if ok { Ok(()) } else { Err(StatusCode::FORBIDDEN) }
    }

    async fn list(
        State(objects): State<Objects>,
        RawQuery(query): RawQuery,
        headers: HeaderMap,
    ) -> Result<String, StatusCode> {
        signed(&headers)?;
        let query: BTreeMap<String, String> = query
            .unwrap_or_default()
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| {
                let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
                (decode(name), decode(value))
            })
            .collect();
        let prefix = query.get("prefix").cloned().unwrap_or_default();
        let after = query.get("continuation-token").cloned().unwrap_or_default();
        let objects = objects.lock().unwrap();
        let mut matching = objects.iter().filter(|(key, _)| key.starts_with(&prefix) && **key > after);
        let page: Vec<_> = matching.by_ref().take(2).collect();
        let more = matching.next().is_some();
        let mut xml = format!("<ListBucketResult><IsTruncated>{more}</IsTruncated>");
        for (key, bytes) in &page {
            let key = key.replace('&', "&amp;");
            xml += &format!("<Contents><Key>{key}</Key><Size>{}</Size></Contents>", bytes.len());
        }
        if more {
            xml += &format!("<NextContinuationToken>{}</NextContinuationToken>", page.last().unwrap().0);
        }
        Ok(xml + "</ListBucketResult>")
    }

    async fn read(
        State(objects): State<Objects>,
        Path(key): Path<String>,
        headers: HeaderMap,
    ) -> Result<Vec<u8>, StatusCode> {
        signed(&headers)?;
        objects.lock().unwrap().get(&key).cloned().ok_or(StatusCode::NOT_FOUND)
    }

    async fn write(
        State(objects): State<Objects>,
        Path(key): Path<String>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<(), StatusCode> {
        signed(&headers)?;
        let length: usize = headers.get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
        if length != body.len() {
            return Err(StatusCode::BAD_REQUEST);
        }
        objects.lock().unwrap().insert(key, body.to_vec());
        Ok(())
    }

    async fn remove(
        State(objects): State<Objects>,
        Path(key): Path<String>,
        headers: HeaderMap,
    ) -> Result<StatusCode, StatusCode> {
        signed(&headers)?;
        objects.lock().unwrap().remove(&key);
        Ok(StatusCode::NO_CONTENT)
    }
}

#[tokio::test]
async fn replicates_to_a_bucket() {
    let (endpoint, objects) = fake_s3::start().await;
    let s3 = S3Config {
        bucket: "bucket".into(),
        endpoint,
        region: "auto".into(),
        access_key_id: "id".into(),
        secret_access_key: "secret".into(),
        path_style: true,
    };
    let dir = tempfile::tempdir().unwrap();
    let (data, out) = (dir.path().join("data"), dir.path().join("out"));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let store = Store::bucket(s3, "/fuwa.chat/").unwrap();
    assert_eq!(store.describe(), "s3://bucket/fuwa.chat/");
    let replica = Replica::with_sizes(store, Duration::from_secs(1), &data, None, 20_000, REBASE_BYTES).unwrap();
    let db = open(&data.join("a.db")).await;
    replica.track("servers/a", db.clone()).await.unwrap();
    for round in 0..6 {
        insert(&db, round * 10, 10, 500).await;
        replica.sync_files().await;
    }
    assert!(objects.lock().unwrap().keys().all(|key| key.starts_with("fuwa.chat/servers/a/")));
    assert!(objects.lock().unwrap().len() > 4, "listing in pages needs more than two objects");
    assert_eq!(restored(replica.store(), "servers/a", &out).await, (60, true));
    assert_eq!(replica.store().get("servers/a/nothing").await.unwrap(), None);
    replica.store().delete("servers/a/current").await.unwrap();
    assert_eq!(replica.store().get("servers/a/current").await.unwrap(), None);
}

#[tokio::test]
async fn a_release_that_fails_is_tried_again_unless_the_server_came_back() {
    let dir = tempfile::tempdir().unwrap();
    let (data, bucket) = (dir.path().join("data"), dir.path().join("bucket"));
    std::fs::create_dir_all(&data).unwrap();
    let replica = replica(&data, &bucket, CHECKPOINT_BYTES, REBASE_BYTES);
    let name = format!("servers/{}", new_id());
    let db = open(&data.join("a.db")).await;
    replica.track(&name, db.clone()).await.unwrap();
    insert(&db, 0, 10, 10).await;
    replica.sync_files().await;
    let kept = async || replica.store().list(&format!("{name}/")).await.unwrap().len();
    assert!(kept().await > 0);
    // The bucket can't be reached: a file stands where it was.
    let away = dir.path().join("away");
    let unreachable = || {
        std::fs::rename(&bucket, &away).unwrap();
        std::fs::write(&bucket, b"").unwrap();
    };
    let reachable = || {
        std::fs::remove_file(&bucket).unwrap();
        std::fs::rename(&away, &bucket).unwrap();
    };

    unreachable();
    replica.release(&name).await;
    assert_eq!(replica.retry_releases().await, 0, "still unreachable");
    reachable();
    assert!(kept().await > 0, "the failed release left it");
    assert_eq!(replica.retry_releases().await, 1);
    assert_eq!(kept().await, 0, "tried again, nothing is left");
    assert_eq!(replica.retry_releases().await, 0, "and it isn't owed any more");

    // A server that came back before the retry keeps its replica.
    replica.track(&name, db.clone()).await.unwrap();
    insert(&db, 10, 10, 10).await;
    replica.sync_files().await;
    unreachable();
    replica.release(&name).await;
    reachable();
    replica.track(&name, db.clone()).await.unwrap();
    replica.sync_files().await;
    assert_eq!(replica.retry_releases().await, 0);
    assert!(kept().await > 0, "the server that came back keeps its replica");
}
