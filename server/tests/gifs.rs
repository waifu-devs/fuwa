//! GIF search end to end: a real instance and a fake GIPHY on this machine
//! (FUWA_GIF_API_URL), checking what reaches the provider, the cache and
//! caps, storing a sent GIF once, seals, saved GIFs and uploaded ones.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::routing::get;
use fuwa_server::app::App;
use fuwa_server::config::Config;
use fuwa_server::pb;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request};

/// What the fake provider saw.
#[derive(Default)]
struct Seen {
    /// Each API call's query string.
    asks: Vec<String>,
    /// Header names of every request, API and files.
    headers: Vec<String>,
    files: usize,
}

type Shared = Arc<Mutex<Seen>>;

/// A 3×2 GIF with a comment (which storing takes out) and looping (which stays).
fn tiny_gif() -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&[3, 0, 2, 0, 0x80, 0, 0, 255, 0, 0, 0, 0, 255]);
    g.extend_from_slice(&[0x21, 0xff, 11]);
    g.extend_from_slice(b"NETSCAPE2.0");
    g.extend_from_slice(&[3, 1, 0, 0, 0]);
    g.extend_from_slice(&[0x21, 0xfe, 9]);
    g.extend_from_slice(b"secret gp");
    g.push(0);
    g.extend_from_slice(&[0x2c, 0, 0, 0, 0, 3, 0, 2, 0, 0, 2, 2, 0x4c, 0x01, 0, 0x3b]);
    g
}

async fn fake_giphy() -> (SocketAddr, Shared) {
    let seen: Shared = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let answer = move |State(seen): State<Shared>, headers: HeaderMap, RawQuery(query): RawQuery| async move {
        let mut seen = seen.lock().unwrap();
        seen.asks.push(query.unwrap_or_default());
        seen.headers.extend(headers.keys().map(|k| k.to_string()));
        let item = |id: &str| {
            let file = format!("http://{addr}/files/{id}.gif");
            serde_json::json!({
                "id": id,
                "title": format!("{id} GIF"),
                "images": {
                    "original": {"url": file, "width": "3", "height": "2", "size": "64"},
                    "fixed_width": {"url": format!("http://{addr}/files/{id}-200.gif"), "width": "200", "height": "133"},
                    "fixed_width_still": {"url": format!("http://{addr}/files/{id}-s.gif"), "width": "200", "height": "133"}
                }
            })
        };
        let body = serde_json::json!({
            "data": [item("one"), item("two")],
            "pagination": {"total_count": 10, "count": 2, "offset": 0}
        });
        ([(axum::http::header::CONTENT_TYPE, "application/json")], body.to_string())
    };
    let files = |State(seen): State<Shared>, headers: HeaderMap| async move {
        let mut seen = seen.lock().unwrap();
        seen.files += 1;
        seen.headers.extend(headers.keys().map(|k| k.to_string()));
        tiny_gif()
    };
    let router = Router::new()
        .route("/v1/gifs/search", get(answer.clone()))
        .route("/v1/gifs/trending", get(answer))
        .route("/files/{name}", get(files))
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (addr, seen)
}

struct Instance {
    app: Arc<App>,
    addr: SocketAddr,
    serving: JoinHandle<()>,
}

async fn start(dir: &Path, giphy: SocketAddr) -> Instance {
    let dir = dir.to_str().unwrap().to_string();
    let config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_GIF_PROVIDER" => Some("giphy".into()),
        "FUWA_GIF_API_KEY" => Some("test-giphy-key-0123".into()),
        "FUWA_GIF_API_URL" => Some(format!("http://{giphy}")),
        _ => None,
    })
    .unwrap();
    let app = App::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    let shutdown = app.shutdown.clone();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    });
    Instance { app, addr, serving }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

fn local(instance: &Instance, url: &str) -> String {
    format!("http://{}{}", instance.addr, reqwest::Url::parse(url).unwrap().path())
}

#[tokio::test]
async fn gifs_are_searched_stored_once_and_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (giphy, seen) = fake_giphy().await;
    let instance = start(dir.path(), giphy).await;
    let channel = Channel::from_shared(format!("http://{}", instance.addr)).unwrap().connect().await.unwrap();
    let mut auth = pb::auth_service_client::AuthServiceClient::new(channel.clone());
    let mut gifs = pb::gif_service_client::GifServiceClient::new(channel.clone());
    let mut servers = pb::server_service_client::ServerServiceClient::new(channel.clone());
    let mut channels = pb::channel_service_client::ChannelServiceClient::new(channel.clone());
    let mut messages = pb::message_service_client::MessageServiceClient::new(channel.clone());
    let mut media = pb::media_service_client::MediaServiceClient::new(channel.clone());
    let mut admin = pb::admin_service_client::AdminServiceClient::new(channel);
    let sign_up = async |auth: &mut pb::auth_service_client::AuthServiceClient<Channel>, name: &str| {
        let res = auth
            .sign_up(pb::SignUpRequest {
                username: name.into(),
                password: "correct horse battery".into(),
                display_name: String::new(),
            })
            .await
            .unwrap()
            .into_inner();
        res.token
    };
    let juan = sign_up(&mut auth, "juan").await;
    let aoi = sign_up(&mut auth, "aoi").await;

    let on = gifs.get_gif_settings(authed(&aoi, pb::GetGifSettingsRequest {})).await.unwrap().into_inner();
    assert!(on.enabled);
    assert_eq!(on.provider, pb::GifProvider::Giphy as i32);

    // Search: pictures come through the instance; the provider hears only the
    // words, and nothing about who asked.
    let search = |query: &str| pb::SearchGifsRequest { query: query.into(), ..Default::default() };
    let found = gifs.search_gifs(authed(&aoi, search("happy  cat"))).await.unwrap().into_inner();
    assert_eq!(found.results.len(), 2);
    assert_eq!(found.next_cursor, "2");
    let first = &found.results[0];
    assert_eq!(first.title, "one GIF");
    assert!(first.preview_url.contains("/media/outside/"), "{}", first.preview_url);
    assert!(first.still_url.contains("/media/outside/"));
    assert_eq!((first.width, first.height), (200, 133));
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.asks.len(), 1);
        assert!(seen.asks[0].contains("q=happy+cat"), "{}", seen.asks[0]);
        assert!(seen.asks[0].contains("api_key=test-giphy-key-0123"));
        for header in &seen.headers {
            assert!(
                !["x-forwarded-for", "forwarded", "x-real-ip", "authorization", "cookie", "referer"]
                    .contains(&header.as_str()),
                "{header} reached the provider"
            );
        }
    }
    // Asked again (any case), it comes from what the instance kept.
    gifs.search_gifs(authed(&juan, search("Happy Cat"))).await.unwrap();
    assert_eq!(seen.lock().unwrap().asks.len(), 1);
    // Trending is an empty search.
    gifs.search_gifs(authed(&aoi, search(""))).await.unwrap();
    assert_eq!(seen.lock().unwrap().asks.len(), 2);
    let long = "x".repeat(101);
    let err = gifs.search_gifs(authed(&aoi, search(&long))).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // Sending: stored once, however many send it.
    let prepare = |id: &str| pb::PrepareGifRequest { from: Some(pb::prepare_gif_request::From::ResultId(id.into())) };
    let gif = gifs.prepare_gif(authed(&aoi, prepare(&first.id))).await.unwrap().into_inner().gif.unwrap();
    assert!(gif.url.contains("/media/") && !gif.url.contains("outside"));
    assert!(!gif.seal.is_empty());
    assert_eq!((gif.width, gif.height, gif.provider), (3, 2, pb::GifProvider::Giphy as i32));
    let again = gifs.prepare_gif(authed(&juan, prepare(&first.id))).await.unwrap().into_inner().gif.unwrap();
    assert_eq!(again.url, gif.url);
    assert_eq!(seen.lock().unwrap().files, 1);
    // A result id the instance didn't make is refused.
    let mut forged = first.id.clone();
    forged.insert(3, 'x');
    let err = gifs.prepare_gif(authed(&aoi, prepare(&forged))).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    // Stored without its comment, still looping.
    let stored = reqwest::get(local(&instance, &gif.url)).await.unwrap().bytes().await.unwrap();
    assert!(stored.starts_with(b"GIF89a"));
    assert!(!stored.windows(9).any(|w| w == b"secret gp"));
    assert!(stored.windows(11).any(|w| w == b"NETSCAPE2.0"));

    // In a message.
    let server = servers
        .create_server(authed(&juan, pb::CreateServerRequest { name: "GIFs".into(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .server
        .unwrap();
    let general = channels
        .list_channels(authed(&juan, pb::ListChannelsRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|c| c.r#type == pb::ChannelType::Text as i32)
        .unwrap();
    let send = |gif: pb::MessageGif| pb::SendMessageRequest {
        server_id: server.id.clone(),
        channel_id: general.id.clone(),
        gif: Some(gif),
        ..Default::default()
    };
    let sent = messages.send_message(authed(&juan, send(again.clone()))).await.unwrap().into_inner().message.unwrap();
    let shown = sent.gif.clone().unwrap();
    assert_eq!(shown.url, gif.url);
    assert!(shown.seal.is_empty(), "the seal isn't kept");
    assert_eq!(shown.title, "one GIF");
    let listed = messages
        .list_messages(authed(
            &juan,
            pb::ListMessagesRequest {
                server_id: server.id.clone(),
                channel_id: general.id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.messages.last().unwrap().gif, Some(shown.clone()));
    // A GIF the instance didn't seal, or one changed after, is refused.
    let changed = pb::MessageGif { title: "something else".into(), ..again.clone() };
    let err = messages.send_message(authed(&juan, send(changed))).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    let elsewhere = pb::MessageGif { url: "https://elsewhere.example/cat.gif".into(), ..again.clone() };
    let err = messages.send_message(authed(&juan, send(elsewhere))).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // Saved GIFs, from a message.
    let saved = gifs.save_gif(authed(&aoi, pb::SaveGifRequest { url: shown.url.clone() })).await.unwrap().into_inner();
    assert!(!saved.gif.unwrap().uploaded);
    let list = gifs.list_saved_gifs(authed(&aoi, pb::ListSavedGifsRequest {})).await.unwrap().into_inner().gifs;
    assert_eq!(list.len(), 1);
    // They come back sealed, ready to send.
    let ready = list[0].gif.clone().unwrap();
    assert!(!ready.seal.is_empty());
    assert!(
        gifs.list_saved_gifs(authed(&juan, pb::ListSavedGifsRequest {})).await.unwrap().into_inner().gifs.is_empty()
    );
    gifs.delete_saved_gif(authed(&aoi, pb::DeleteSavedGifRequest { url: ready.url.clone() })).await.unwrap();
    assert!(
        gifs.list_saved_gifs(authed(&aoi, pb::ListSavedGifsRequest {})).await.unwrap().into_inner().gifs.is_empty()
    );
    // The file stays for the message.
    assert!(reqwest::get(local(&instance, &gif.url)).await.unwrap().status().is_success());

    // A GIF of your own: only a GIF, then sent like any other.
    let reserve = |content_type: &str, size: usize| pb::CreateUploadRequest {
        purpose: pb::MediaPurpose::Gif as i32,
        content_type: content_type.into(),
        size: size as i64,
        ..Default::default()
    };
    let err = media.create_upload(authed(&aoi, reserve("image/png", 100))).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    let own = tiny_gif();
    let reserved = media.create_upload(authed(&aoi, reserve("image/gif", own.len()))).await.unwrap().into_inner();
    let put = reqwest::Client::new().put(local(&instance, &reserved.upload_url)).body(own).send().await.unwrap();
    assert!(put.status().is_success());
    let upload_url = reserved.media.unwrap().url;
    let mine = gifs.save_gif(authed(&aoi, pb::SaveGifRequest { url: upload_url.clone() })).await.unwrap().into_inner();
    let mine = mine.gif.unwrap();
    assert!(mine.uploaded);
    assert_eq!(mine.gif.as_ref().unwrap().provider, pb::GifProvider::Unspecified as i32);
    // Someone else's upload isn't theirs to send.
    let theirs = pb::PrepareGifRequest { from: Some(pb::prepare_gif_request::From::UploadUrl(upload_url.clone())) };
    assert!(gifs.prepare_gif(authed(&juan, theirs.clone())).await.is_err());
    let own_gif = gifs.prepare_gif(authed(&aoi, theirs)).await.unwrap().into_inner().gif.unwrap();
    assert_eq!(own_gif.url, upload_url);

    // Admins see whether a key is set, never the key.
    let config = admin.get_settings(authed(&juan, pb::GetSettingsRequest {})).await.unwrap().into_inner();
    let shown_gifs = config.config.unwrap().settings.unwrap().gifs.unwrap();
    assert!(shown_gifs.api_key.is_empty() && shown_gifs.api_key_set);
    assert_eq!(shown_gifs.api_key_hint, "0123");

    // Caps: searches a minute per account, and provider calls a day.
    let update = |gifs: pb::GifSettings| pb::UpdateSettingsRequest {
        settings: Some(pb::InstanceSettings { gifs: Some(gifs), ..Default::default() }),
        update_mask: Some(prost_types::FieldMask { paths: vec!["gifs".into()] }),
        reset_mask: None,
    };
    let capped = pb::GifSettings {
        provider: pb::GifProvider::Giphy as i32,
        searches_per_minute: Some(1),
        provider_calls_per_day: Some(0),
        ..Default::default()
    };
    admin.update_settings(authed(&juan, update(capped))).await.unwrap();
    // Kept from before: no provider call, so the daily cap doesn't hold it back.
    gifs.search_gifs(authed(&juan, search("happy cat"))).await.unwrap();
    let err = gifs.search_gifs(authed(&juan, search("happy cat"))).await.unwrap_err();
    assert_eq!(err.code(), Code::ResourceExhausted, "one a minute");
    let err = gifs.search_gifs(authed(&aoi, search("dogs"))).await.unwrap_err();
    assert_eq!(err.code(), Code::ResourceExhausted, "no provider calls left today");
    assert!(err.message().contains("today"));

    // Off: no search, but saved GIFs still send.
    let off = pb::GifSettings { provider: pb::GifProvider::Unspecified as i32, ..Default::default() };
    admin.update_settings(authed(&juan, update(off))).await.unwrap();
    let off = gifs.get_gif_settings(authed(&aoi, pb::GetGifSettingsRequest {})).await.unwrap().into_inner();
    assert!(!off.enabled);
    let err = gifs.search_gifs(authed(&aoi, search("cat"))).await.unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition);
    messages.send_message(authed(&juan, send(again))).await.unwrap();

    instance.app.shutdown.cancel();
    instance.serving.await.unwrap();
}
