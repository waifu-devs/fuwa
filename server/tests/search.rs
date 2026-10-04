//! Searching servers' messages, end to end: a real instance, its indexer
//! following the event log, and SearchService through the gRPC clients.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request};

struct Instance {
    app: Arc<App>,
    addr: std::net::SocketAddr,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        _ => None,
    })
    .unwrap();
    let app = App::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    let shutdown = app.shutdown.clone();
    fuwa_server::api::spawn_search_indexer(app.clone());
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    });
    Instance { app, addr, serving }
}

impl Instance {
    async fn stop(self) {
        self.app.shutdown.cancel();
        self.serving.await.unwrap();
    }

    /// Waits until the server's index has taken in every event and every
    /// older message, reading its state from the file (searching to find out
    /// would count against the rate limit).
    async fn indexed(&self, server_id: &str) {
        let sdb = self.app.servers.get(server_id).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let head = sdb.head_sequence().await.unwrap();
            let conn = sdb.read().unwrap();
            let mut rows = conn.query("SELECT sequence, backfill IS NULL FROM search_state", ()).await.unwrap();
            let row = rows.next().await.unwrap().unwrap();
            let (sequence, built) = (row.get::<i64>(0).unwrap(), row.get::<i64>(1).unwrap() == 1);
            if sequence == head && built {
                return;
            }
            assert!(Instant::now() < deadline, "the index didn't catch up (at {sequence} of {head})");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    servers: pb::server_service_client::ServerServiceClient<Channel>,
    channels: pb::channel_service_client::ChannelServiceClient<Channel>,
    messages: pb::message_service_client::MessageServiceClient<Channel>,
    search: pb::search_service_client::SearchServiceClient<Channel>,
}

async fn clients(instance: &Instance) -> Clients {
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        servers: pb::server_service_client::ServerServiceClient::new(channel.clone()),
        channels: pb::channel_service_client::ChannelServiceClient::new(channel.clone()),
        messages: pb::message_service_client::MessageServiceClient::new(channel.clone()),
        search: pb::search_service_client::SearchServiceClient::new(channel),
    }
}

async fn sign_up(c: &mut Clients, username: &str) -> (String, pb::User) {
    let res = c
        .auth
        .sign_up(pb::SignUpRequest {
            username: username.into(),
            password: "correct horse battery".into(),
            display_name: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    (res.token, res.user.unwrap())
}

async fn send(c: &mut Clients, token: &str, server_id: &str, channel_id: &str, content: &str) -> pb::Message {
    c.messages
        .send_message(authed(
            token,
            pb::SendMessageRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                content: content.into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .message
        .unwrap()
}

/// A search, waiting out the rate limit when a test goes faster than people do.
async fn search(c: &mut Clients, token: &str, req: pb::SearchMessagesRequest) -> pb::SearchMessagesResponse {
    loop {
        match c.search.search_messages(authed(token, req.clone())).await {
            Ok(res) => return res.into_inner(),
            Err(err) if err.code() == Code::ResourceExhausted => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(err) => panic!("search failed: {err}"),
        }
    }
}

fn contents(res: &pb::SearchMessagesResponse) -> Vec<String> {
    res.results.iter().map(|r| r.message.as_ref().unwrap().content.clone()).collect()
}

#[tokio::test]
async fn searching_a_server() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let (mika, mika_user) = sign_up(&mut c, "mika").await;
    let server = c
        .servers
        .create_server(authed(
            &juan,
            pb::CreateServerRequest { name: "Search".into(), discoverable: true, ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let sid = server.id.clone();
    c.servers
        .join_server(authed(&mika, pb::JoinServerRequest { server_id: sid.clone(), ..Default::default() }))
        .await
        .unwrap();
    let general = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels[0]
        .id
        .clone();
    // A channel only juan can see.
    let secret = c
        .channels
        .create_channel(authed(
            &juan,
            pb::CreateChannelRequest {
                server_id: sid.clone(),
                name: "secret".into(),
                r#type: pb::ChannelType::Text as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap()
        .id;
    c.channels
        .set_channel_permissions(authed(
            &juan,
            pb::SetChannelPermissionsRequest {
                server_id: sid.clone(),
                channel_id: secret.clone(),
                overwrites: vec![pb::PermissionOverwrite {
                    target_id: sid.clone(),
                    target: pb::OverwriteTarget::Role as i32,
                    allow: vec![],
                    deny: vec![pb::Permission::ViewChannels as i32],
                }],
            },
        ))
        .await
        .unwrap();

    let hello = send(&mut c, &juan, &sid, &general, "Hello wonderful Café world").await;
    send(&mut c, &mika, &sid, &general, "hey @juan look at https://example.com").await;
    send(&mut c, &juan, &sid, &secret, "secret hello plans").await;
    send(&mut c, &mika, &sid, &general, "東京都に行きます").await;
    instance.indexed(&sid).await;

    let q =
        |query: &str| pb::SearchMessagesRequest { server_id: sid.clone(), query: query.into(), ..Default::default() };

    // Case and accents don't matter, and hidden channels don't show, not even in the total.
    let res = search(&mut c, &mika, q("hello cafe")).await;
    assert_eq!(contents(&res), ["Hello wonderful Café world"]);
    assert_eq!(res.total, 1);
    assert!(!res.indexing);
    let highlights: Vec<(i32, i32)> = res.results[0].highlights.iter().map(|h| (h.start, h.end)).collect();
    assert_eq!(highlights, [(0, 5), (16, 20)]);
    assert_eq!(res.authors.len(), 1);
    let res = search(&mut c, &juan, q("hello")).await;
    assert_eq!(contents(&res), ["secret hello plans", "Hello wonderful Café world"], "newest first");
    assert_eq!(res.total, 2);
    // Asking for the hidden channel by id finds nothing either.
    let res =
        search(&mut c, &mika, pb::SearchMessagesRequest { channel_ids: vec![secret.clone()], ..q("hello") }).await;
    assert!(res.results.is_empty() && res.total == 0);

    // The last word typed matches longer words; with a space after, only itself.
    assert_eq!(search(&mut c, &mika, q("hello wond")).await.total, 1);
    assert_eq!(search(&mut c, &mika, q("hello wond ")).await.total, 0);
    // Japanese runs are found by any two characters in a row.
    assert_eq!(contents(&search(&mut c, &mika, q("京都")).await), ["東京都に行きます"]);

    // Filters, with words and on their own.
    let res =
        search(&mut c, &juan, pb::SearchMessagesRequest { author_ids: vec![mika_user.id.clone()], ..q("") }).await;
    assert_eq!(res.total, 2);
    let res =
        search(&mut c, &juan, pb::SearchMessagesRequest { mention_ids: vec![hello.author_id.clone()], ..q("") }).await;
    assert_eq!(res.total, 1);
    let res =
        search(&mut c, &juan, pb::SearchMessagesRequest { has: vec![pb::SearchHas::Link as i32], ..q("look") }).await;
    assert_eq!(res.total, 1);
    let res = search(&mut c, &juan, pb::SearchMessagesRequest { channel_ids: vec![secret.clone()], ..q("") }).await;
    assert_eq!(contents(&res), ["secret hello plans"]);
    let sent = hello.created_at.unwrap();
    let res = search(&mut c, &juan, pb::SearchMessagesRequest { before: Some(sent), ..q("hello") }).await;
    assert_eq!(res.total, 0, "before is up to, not including");
    let res = search(&mut c, &juan, pb::SearchMessagesRequest { after: Some(sent), ..q("hello") }).await;
    assert_eq!(res.total, 2);

    // Edits and deletes reach the index.
    c.messages
        .update_message(authed(
            &juan,
            pb::UpdateMessageRequest {
                server_id: sid.clone(),
                message_id: hello.id.clone(),
                content: "Goodbye moon".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    instance.indexed(&sid).await;
    assert_eq!(search(&mut c, &mika, q("wonderful")).await.total, 0);
    assert_eq!(contents(&search(&mut c, &mika, q("moon")).await), ["Goodbye moon"]);
    c.messages
        .delete_message(authed(
            &juan,
            pb::DeleteMessageRequest { server_id: sid.clone(), message_id: hello.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap();
    instance.indexed(&sid).await;
    assert_eq!(search(&mut c, &mika, q("moon")).await.total, 0);

    // A deleted channel takes its messages out of the index.
    c.channels
        .delete_channel(authed(&juan, pb::DeleteChannelRequest { server_id: sid.clone(), channel_id: secret.clone() }))
        .await
        .unwrap();
    instance.indexed(&sid).await;
    assert_eq!(search(&mut c, &juan, q("plans")).await.total, 0);
    let sdb = instance.app.servers.get(&sid).await.unwrap();
    let mut rows = sdb
        .read()
        .unwrap()
        .query("SELECT count(*) FROM search_docs WHERE channel_id = ?1", [secret.as_str()])
        .await
        .unwrap();
    assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(), 0);

    // An empty search is refused, and strangers can't search.
    let err = c.search.search_messages(authed(&juan, q("  "))).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    let (stranger, _) = sign_up(&mut c, "stranger").await;
    let err = c.search.search_messages(authed(&stranger, q("hello"))).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);

    // Each account searches a few times a second at most.
    let mut limited = false;
    for _ in 0..20 {
        if let Err(err) = c.search.search_messages(authed(&stranger, q("hello"))).await
            && err.code() == Code::ResourceExhausted
        {
            limited = true;
        }
        let _ = c.search.search_messages(authed(&mika, q("look"))).await.map_err(|err| {
            if err.code() == Code::ResourceExhausted {
                limited = true;
            }
        });
    }
    assert!(limited);
    instance.stop().await;
}

#[tokio::test]
async fn pages_and_older_messages() {
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let server = c
        .servers
        .create_server(authed(&juan, pb::CreateServerRequest { name: "Pages".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let sid = server.id.clone();
    let general = c
        .channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: sid.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels[0]
        .id
        .clone();
    for n in 0..30 {
        send(&mut c, &juan, &sid, &general, &format!("note number {n}")).await;
    }
    instance.indexed(&sid).await;

    // Pages: 12, 12, then 6, newest first with no repeats.
    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let res = search(
            &mut c,
            &juan,
            pb::SearchMessagesRequest {
                server_id: sid.clone(),
                query: "note".into(),
                limit: 12,
                cursor: cursor.clone(),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(res.total, if cursor.is_empty() { 30 } else { 0 });
        seen.extend(contents(&res));
        cursor = res.next_cursor;
        if cursor.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    let expected: Vec<String> = (0..30).rev().map(|n| format!("note number {n}")).collect();
    assert_eq!(seen, expected);

    // A server from before search: its index is empty until its older
    // messages are added in the background, after the next start.
    let sdb = instance.app.servers.get(&sid).await.unwrap();
    sdb.write_quiet(async |conn| {
        conn.execute_batch(
            "DELETE FROM search_postings; DELETE FROM search_docs; DELETE FROM search_words;
             UPDATE search_state SET backfill = '~'",
        )
        .await?;
        Ok(())
    })
    .await
    .unwrap();
    drop(sdb);
    instance.stop().await;
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    instance.indexed(&sid).await;
    let res = search(
        &mut c,
        &juan,
        pb::SearchMessagesRequest { server_id: sid.clone(), query: "number 17".into(), ..Default::default() },
    )
    .await;
    assert_eq!(contents(&res), ["note number 17"]);
    assert!(!res.indexing);
    instance.stop().await;
}

/// Measures a server with 100,000 messages: building its index from scratch
/// and searching it. Run with
/// `cargo test --release -p fuwa-server --test search -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn hundred_thousand_messages() {
    const MESSAGES: usize = 100_000;
    let dir = tempfile::tempdir().unwrap();
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let (juan, juan_user) = sign_up(&mut c, "juan").await;
    let server = c
        .servers
        .create_server(authed(&juan, pb::CreateServerRequest { name: "Big".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let sid = server.id.clone();
    let mut channels = vec![];
    for name in ["general", "dev", "art", "music", "games", "offtopic", "help", "news"] {
        let channel = c
            .channels
            .create_channel(authed(
                &juan,
                pb::CreateChannelRequest {
                    server_id: sid.clone(),
                    name: name.into(),
                    r#type: pb::ChannelType::Text as i32,
                    ..Default::default()
                },
            ))
            .await
            .unwrap()
            .into_inner()
            .channel
            .unwrap();
        channels.push(channel.id);
    }

    // Chat-like text: words drawn so a few are very common and most are rare.
    let vocabulary: Vec<String> = {
        let common = [
            "the", "and", "you", "that", "this", "for", "with", "have", "just", "like", "what", "lol", "yeah",
            "anyone", "know",
        ];
        let mut words: Vec<String> = common.iter().map(|w| w.to_string()).collect();
        let syllables =
            ["ka", "ri", "mo", "to", "na", "shi", "ru", "fu", "wa", "ne", "ko", "ya", "mi", "su", "te", "ho"];
        let mut seed = 7u64;
        while words.len() < 20_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let len = 2 + (seed >> 60) as usize % 3;
            let word: String = (0..len).map(|i| syllables[((seed >> (i * 8)) & 15) as usize]).collect();
            words.push(word);
        }
        words
    };
    let mut seed = 42u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        seed >> 33
    };
    let sdb = instance.app.servers.get(&sid).await.unwrap();
    let started = Instant::now();
    let base_ms = 1_700_000_000_000i64;
    let mut batch = Vec::new();
    for n in 0..MESSAGES {
        let words = 4 + next() as usize % 14;
        let text: Vec<&str> = (0..words)
            .map(|_| {
                // Zipf-ish: squaring a uniform number favours the front.
                let u = (next() % 10_000) as f64 / 10_000.0;
                vocabulary[((u * u * u) * vocabulary.len() as f64) as usize].as_str()
            })
            .collect();
        let mut content = text.join(" ");
        if n % 50 == 0 {
            content.push_str(" https://example.com/page");
        }
        let ms = base_ms + n as i64 * 30_000;
        let id = ulid::Ulid::from_parts(ms as u64, u128::from(next()) << 40 | n as u128).to_string();
        batch.push((id, channels[next() as usize % channels.len()].clone(), content, ms));
        if batch.len() == 1000 {
            let rows = std::mem::take(&mut batch);
            let author = juan_user.id.clone();
            sdb.write_quiet(async |conn| {
                for (id, channel, content, ms) in &rows {
                    conn.execute(
                        "INSERT INTO messages (id, channel_id, author_id, content, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        (id.as_str(), channel.as_str(), author.as_str(), content.as_str(), content.len() as i64, *ms),
                    )
                    .await?;
                }
                Ok(())
            })
            .await
            .unwrap();
        }
    }
    sdb.write_quiet(async |conn| {
        conn.execute("UPDATE search_state SET backfill = '~'", ()).await?;
        Ok(())
    })
    .await
    .unwrap();
    println!("wrote {MESSAGES} messages in {:?}", started.elapsed());
    let before = sdb.storage_bytes();
    drop(sdb);
    instance.stop().await;

    // The index builds in the background after a start.
    let instance = start(dir.path()).await;
    let mut c = clients(&instance).await;
    let started = Instant::now();
    instance.indexed(&sid).await;
    let built = started.elapsed();
    let sdb = instance.app.servers.get(&sid).await.unwrap();
    sdb.db().checkpoint().await.unwrap();
    let after = sdb.storage_bytes();
    println!("built the index of {MESSAGES} messages in {built:?}");
    println!("file: {:.1} MB before, {:.1} MB with the index", before as f64 / 1e6, after as f64 / 1e6);

    // While the index builds, the shard keeps answering: a send right now is quick.
    let queries: Vec<(&str, pb::SearchMessagesRequest)> = vec![
        ("rare word", pb::SearchMessagesRequest { query: vocabulary[15_000].clone(), ..Default::default() }),
        ("common word", pb::SearchMessagesRequest { query: "the".into(), ..Default::default() }),
        ("two common words", pb::SearchMessagesRequest { query: "the and".into(), ..Default::default() }),
        ("prefix", pb::SearchMessagesRequest { query: "kari".into(), ..Default::default() }),
        (
            "common word in one channel",
            pb::SearchMessagesRequest {
                query: "you".into(),
                channel_ids: vec![channels[1].clone()],
                ..Default::default()
            },
        ),
        (
            "has link, no words",
            pb::SearchMessagesRequest { has: vec![pb::SearchHas::Link as i32], ..Default::default() },
        ),
        (
            "one channel, no words",
            pb::SearchMessagesRequest { channel_ids: vec![channels[2].clone()], ..Default::default() },
        ),
    ];
    for (name, mut req) in queries {
        req.server_id = sid.clone();
        let mut times = vec![];
        let mut total = 0;
        for _ in 0..5 {
            let started = Instant::now();
            let res = search(&mut c, &juan, req.clone()).await;
            times.push(started.elapsed());
            total = res.total;
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        times.sort();
        println!("{name}: {total} results, median {:?}, worst {:?}", times[2], times[4]);
    }
    instance.stop().await;
}
