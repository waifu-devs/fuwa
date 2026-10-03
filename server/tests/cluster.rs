//! A split instance end to end: a directory, two shards and a gateway, each
//! on its own local port, driven through the gateway as clients would.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fuwa_server::app::App;
use fuwa_server::cluster::gateway::Gateway;
use fuwa_server::config::Config;
use fuwa_server::pb;
use fuwa_server::pb::event::Payload;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tonic::{Code, Request, Streaming};

const KEY: &str = "cluster-key-0123456789abcdef0123456789abcdef";
const ADMIN_TOKEN: &str = "test-admin-token-0123456789abcdef0123456789";
const PASSWORD: &str = "correct horse battery";

/// One running part.
struct Part {
    app: Option<Arc<App>>,
    addr: SocketAddr,
    shutdown: CancellationToken,
    serving: JoinHandle<()>,
}

impl Part {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn app(&self) -> &Arc<App> {
        self.app.as_ref().unwrap()
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        self.serving.await.unwrap();
        self.app.take();
    }
}

/// Stands in for a part that was stopped, until it starts again.
fn stopped(addr: SocketAddr) -> Part {
    Part { app: None, addr, shutdown: CancellationToken::new(), serving: tokio::spawn(async {}) }
}

async fn listen() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

/// How long parts wait for one another to restart here: long enough for a
/// part to stop and start again in this process, short enough to see one
/// that stays down.
const RIDE_OUT: Duration = Duration::from_secs(3);

fn config(dir: &Path, vars: &[(&str, String)]) -> Config {
    let dir = dir.to_str().unwrap().to_string();
    let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let mut config = Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_ADMIN_TOKEN" => Some(ADMIN_TOKEN.into()),
        "FUWA_CLUSTER_KEY" => Some(KEY.into()),
        _ => vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
    })
    .unwrap();
    config.cluster.ride_out = RIDE_OUT;
    config
}

fn serve(listener: TcpListener, router: axum::Router, shutdown: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    })
}

async fn start_app(config: Config, listener: TcpListener, addr: SocketAddr) -> Part {
    let app = App::open(config).await.unwrap();
    let shutdown = app.shutdown.clone();
    let serving = serve(listener, app.router(), shutdown.clone());
    Part { app: Some(app), addr, shutdown, serving }
}

async fn start_shard(dir: &Path, name: &str, directory: &Part) -> Part {
    start_shard_with(dir, name, directory, &[]).await
}

async fn start_shard_with(dir: &Path, name: &str, directory: &Part, more: &[(&str, String)]) -> Part {
    let (listener, addr) = listen().await;
    let mut vars = vec![
        ("FUWA_ROLE", "shard".to_string()),
        ("FUWA_SHARD_ID", name.to_string()),
        ("FUWA_DIRECTORY_URL", directory.url()),
        ("FUWA_INTERNAL_URL", format!("http://{addr}")),
    ];
    vars.extend(more.iter().cloned());
    start_app(config(dir, &vars), listener, addr).await
}

struct Cluster {
    directory: Part,
    shards: Vec<Part>,
    gateway: Part,
    _gateway: Arc<Gateway>,
}

async fn start_cluster(root: &Path, vars: &[(&str, String)]) -> Cluster {
    let (gateway_listener, gateway_addr) = listen().await;
    let (listener, addr) = listen().await;
    let directory = start_directory(root, vars, listener, addr, gateway_addr).await;
    let shards = vec![
        start_shard(&root.join("shard-a"), "a", &directory).await,
        start_shard(&root.join("shard-b"), "b", &directory).await,
    ];
    let (gateway, _gateway) = start_gateway(&root.join("gateway"), &directory, gateway_listener, gateway_addr);
    Cluster { directory, shards, gateway, _gateway }
}

async fn start_directory(
    root: &Path,
    vars: &[(&str, String)],
    listener: TcpListener,
    addr: SocketAddr,
    gateway_addr: SocketAddr,
) -> Part {
    let mut directory_vars =
        vec![("FUWA_ROLE", "directory".to_string()), ("FUWA_PUBLIC_URL", format!("http://{gateway_addr}"))];
    directory_vars.extend(vars.iter().cloned());
    start_app(config(&root.join("directory"), &directory_vars), listener, addr).await
}

fn start_gateway(dir: &Path, directory: &Part, listener: TcpListener, addr: SocketAddr) -> (Part, Arc<Gateway>) {
    let vars = [("FUWA_ROLE", "gateway".to_string()), ("FUWA_DIRECTORY_URL", directory.url())];
    let gateway = Gateway::new(config(dir, &vars)).unwrap();
    let shutdown = gateway.shutdown();
    let serving = serve(listener, gateway.router(), shutdown.clone());
    (Part { app: None, addr, shutdown, serving }, gateway)
}

impl Cluster {
    async fn stop(self) {
        self.gateway.stop().await;
        for shard in self.shards {
            shard.stop().await;
        }
        self.directory.stop().await;
    }

    /// Which shard holds a server, as the directory knows it.
    fn placement(&self, server_id: &str) -> Option<String> {
        self.directory.app().index.placement(server_id)
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    account: pb::account_service_client::AccountServiceClient<Channel>,
    servers: pb::server_service_client::ServerServiceClient<Channel>,
    channels: pb::channel_service_client::ChannelServiceClient<Channel>,
    messages: pb::message_service_client::MessageServiceClient<Channel>,
    events: pb::event_service_client::EventServiceClient<Channel>,
    admin: pb::admin_service_client::AdminServiceClient<Channel>,
    node: pb::node_service_client::NodeServiceClient<Channel>,
    media: pb::media_service_client::MediaServiceClient<Channel>,
    invites: pb::invite_service_client::InviteServiceClient<Channel>,
    webhooks: pb::webhook_service_client::WebhookServiceClient<Channel>,
    agents: pb::agent_service_client::AgentServiceClient<Channel>,
    shared: pb::shared_channel_service_client::SharedChannelServiceClient<Channel>,
    automod: pb::auto_mod_service_client::AutoModServiceClient<Channel>,
}

async fn clients(part: &Part) -> Clients {
    let channel = Channel::from_shared(part.url()).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        account: pb::account_service_client::AccountServiceClient::new(channel.clone()),
        servers: pb::server_service_client::ServerServiceClient::new(channel.clone()),
        channels: pb::channel_service_client::ChannelServiceClient::new(channel.clone()),
        messages: pb::message_service_client::MessageServiceClient::new(channel.clone()),
        events: pb::event_service_client::EventServiceClient::new(channel.clone()),
        admin: pb::admin_service_client::AdminServiceClient::new(channel.clone()),
        node: pb::node_service_client::NodeServiceClient::new(channel.clone()),
        media: pb::media_service_client::MediaServiceClient::new(channel.clone()),
        invites: pb::invite_service_client::InviteServiceClient::new(channel.clone()),
        webhooks: pb::webhook_service_client::WebhookServiceClient::new(channel.clone()),
        agents: pb::agent_service_client::AgentServiceClient::new(channel.clone()),
        shared: pb::shared_channel_service_client::SharedChannelServiceClient::new(channel.clone()),
        automod: pb::auto_mod_service_client::AutoModServiceClient::new(channel),
    }
}

async fn sign_up(c: &mut Clients, username: &str) -> (String, pb::User) {
    let request =
        pb::SignUpRequest { username: username.into(), password: PASSWORD.into(), display_name: String::new() };
    let res = c.auth.sign_up(request).await.unwrap().into_inner();
    (res.token, res.user.unwrap())
}

async fn create_server(c: &mut Clients, token: &str, name: &str) -> pb::Server {
    let request = pb::CreateServerRequest { name: name.into(), discoverable: true, ..Default::default() };
    c.servers.create_server(authed(token, request)).await.unwrap().into_inner().server.unwrap()
}

async fn general(c: &mut Clients, token: &str, server_id: &str) -> pb::Channel {
    let request = pb::ListChannelsRequest { server_id: server_id.into() };
    c.channels.list_channels(authed(token, request)).await.unwrap().into_inner().channels.remove(0)
}

async fn send(c: &mut Clients, token: &str, server_id: &str, channel_id: &str, content: &str) -> pb::Message {
    let request = pb::SendMessageRequest {
        server_id: server_id.into(),
        channel_id: channel_id.into(),
        content: content.into(),
        ..Default::default()
    };
    c.messages.send_message(authed(token, request)).await.unwrap().into_inner().message.unwrap()
}

async fn join(c: &mut Clients, token: &str, server_id: &str) -> Result<pb::JoinServerResponse, tonic::Status> {
    c.servers
        .join_server(authed(token, pb::JoinServerRequest { server_id: server_id.into(), ..Default::default() }))
        .await
        .map(|r| r.into_inner())
}

async fn invite(c: &mut Clients, token: &str, server_id: &str) -> pb::Invite {
    let request = pb::CreateInviteRequest { server_id: server_id.into(), ..Default::default() };
    c.invites.create_invite(authed(token, request)).await.unwrap().into_inner().invite.unwrap()
}

async fn members(c: &mut Clients, token: &str, server_id: &str) -> Vec<pb::Member> {
    let request = pb::ListMembersRequest { server_id: server_id.into() };
    c.servers.list_members(authed(token, request)).await.unwrap().into_inner().members
}

async fn next(stream: &mut Streaming<pb::SubscribeResponse>) -> pb::SubscribeResponse {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), stream.next()).await.unwrap().unwrap().unwrap();
        if message.event.is_some() || message.ready.is_some() {
            return message;
        }
    }
}

/// Reads events until one matches.
async fn until(stream: &mut Streaming<pb::SubscribeResponse>, matches: impl Fn(&pb::Event) -> bool) -> pb::Event {
    loop {
        if let Some(event) = next(stream).await.event
            && matches(&event)
        {
            return event;
        }
    }
}

/// Retries until a call stops failing as `code`, for changes that take a
/// moment to reach every part.
/// Waits for something done in the background.
async fn wait_for(done: impl Fn() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(done(), "it didn't happen in time");
}

async fn eventually<T, F: Future<Output = Result<T, tonic::Status>>>(mut call: impl FnMut() -> F) -> T {
    for _ in 0..100 {
        match call().await {
            Ok(value) => return value,
            Err(status) if matches!(status.code(), Code::Unavailable | Code::NotFound) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(status) => panic!("{status:?}"),
        }
    }
    panic!("still failing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_split_instance_works_like_one() {
    let root = tempfile::tempdir().unwrap();
    let cluster = start_cluster(root.path(), &[("FUWA_NODE_NAME", "Split".to_string())]).await;
    let mut c = clients(&cluster.gateway).await;

    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    assert_eq!(node.name, "Split");
    let (juan, juan_user) = sign_up(&mut c, "juan").await;
    let (mika, mika_user) = sign_up(&mut c, "mika").await;

    // New servers go to the shard holding the fewest.
    let mut servers = Vec::new();
    for name in ["One", "Two", "Three", "Four"] {
        servers.push(create_server(&mut c, &juan, name).await);
    }
    let shards: Vec<String> = servers.iter().map(|s| cluster.placement(&s.id).unwrap()).collect();
    assert_eq!(shards.iter().filter(|s| *s == "a").count(), 2);
    assert_eq!(shards.iter().filter(|s| *s == "b").count(), 2);
    let on_a = servers[shards.iter().position(|s| s == "a").unwrap()].clone();
    let on_b = servers[shards.iter().position(|s| s == "b").unwrap()].clone();
    assert!(cluster.shards[0].app().servers.holds(&on_a.id) && !cluster.shards[1].app().servers.holds(&on_a.id));

    // Joining servers on both shards; the directory's lists keep up.
    let discovered =
        c.servers.discover_servers(authed(&mika, pb::DiscoverServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(discovered.len(), 4);
    let joined = join(&mut c, &mika, &on_a.id).await.unwrap();
    assert_eq!(joined.server.unwrap().member_count, 2);
    join(&mut c, &mika, &on_b.id).await.unwrap();
    assert_eq!(join(&mut c, &mika, &on_b.id).await.unwrap_err().code(), Code::AlreadyExists);
    let mine = c.servers.list_servers(authed(&mika, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(
        mine.iter().map(|s| s.id.as_str()).collect::<HashSet<_>>(),
        HashSet::from([on_a.id.as_str(), on_b.id.as_str()])
    );
    assert!(mine.iter().all(|s| s.member_count == 2));

    // Invites: made on a shard, found through the directory, used on the shard.
    let (rin, _) = sign_up(&mut c, "rin").await;
    let code = invite(&mut c, &juan, &on_b.id).await.code;
    let shown = c.invites.get_invite(pb::GetInviteRequest { code: code.clone() }).await.unwrap().into_inner();
    assert_eq!(shown.server.unwrap().id, on_b.id);
    assert_eq!(shown.inviter.unwrap().id, juan_user.id);
    let request = pb::JoinServerRequest { server_id: on_b.id.clone(), invite_code: code.clone() };
    c.servers.join_server(authed(&rin, request)).await.unwrap();
    let shown = c.invites.get_invite(pb::GetInviteRequest { code: code.clone() }).await.unwrap().into_inner();
    assert_eq!(shown.invite.unwrap().uses, 1);
    let request = pb::DeleteInviteRequest { server_id: on_b.id.clone(), code: code.clone() };
    c.invites.delete_invite(authed(&juan, request)).await.unwrap();
    let gone = c.invites.get_invite(pb::GetInviteRequest { code }).await.unwrap_err();
    assert_eq!(gone.code(), Code::NotFound);

    // Unknown and malformed servers read the same as on one process.
    let missing = c
        .servers
        .get_server(authed(&mika, pb::GetServerRequest { server_id: "01J00000000000000000000000".into() }))
        .await;
    assert_eq!(missing.unwrap_err().code(), Code::NotFound);
    let malformed = c.servers.get_server(authed(&mika, pb::GetServerRequest { server_id: "../x".into() })).await;
    assert_eq!(malformed.unwrap_err().code(), Code::InvalidArgument);

    // One live stream follows servers on both shards: replays, one ready, then live.
    let cursors =
        [&on_a, &on_b].iter().map(|s| pb::ServerCursor { server_id: s.id.clone(), after_sequence: Some(0) }).collect();
    let mut stream =
        c.events.subscribe(authed(&mika, pb::SubscribeRequest { servers: cursors })).await.unwrap().into_inner();
    let mut replayed = HashSet::new();
    let ready = loop {
        let message = next(&mut stream).await;
        match (message.event, message.ready) {
            (Some(event), _) => {
                replayed.insert(event.server_id);
            }
            (None, Some(ready)) => break ready,
            _ => unreachable!(),
        }
    };
    assert_eq!(replayed.len(), 2);
    assert_eq!(ready.servers.len(), 2);
    assert!(ready.servers.iter().all(|head| head.sequence > 0));

    let channel_a = general(&mut c, &juan, &on_a.id).await;
    let channel_b = general(&mut c, &juan, &on_b.id).await;
    send(&mut c, &juan, &on_a.id, &channel_a.id, "hello from a").await;
    send(&mut c, &juan, &on_b.id, &channel_b.id, "hello from b").await;
    let mut seen = HashSet::new();
    while seen.len() < 2 {
        let event = until(&mut stream, |e| matches!(e.payload, Some(Payload::MessageCreated(_)))).await;
        seen.insert(event.server_id);
    }
    send(&mut c, &mika, &on_b.id, &channel_b.id, "mika was here").await;
    let listed = c
        .events
        .list_events(authed(&mika, pb::ListEventsRequest { server_id: on_b.id.clone(), ..Default::default() }))
        .await
        .unwrap()
        .into_inner();
    assert!(listed.events.len() >= 4);

    // A new profile reaches every server, on every shard.
    let rename = pb::UpdateProfileRequest { display_name: Some("Mika ✨".into()), ..Default::default() };
    c.auth.update_profile(authed(&mika, rename)).await.unwrap();
    for server in [&on_a, &on_b] {
        let found = members(&mut c, &juan, &server.id).await;
        assert!(found.iter().any(|m| m.user.as_ref().unwrap().display_name == "Mika ✨"), "on {}", server.name);
    }
    until(&mut stream, |e| matches!(&e.payload, Some(Payload::MemberUpdated(_)))).await;
    let profile = c.auth.get_profile(authed(&mika, pb::GetProfileRequest { user_id: juan_user.id.clone() })).await;
    assert!(profile.is_ok());

    // Notification settings check the channel on its shard.
    let change = |channel_id: &str| pb::UpdateNotificationSettingsRequest {
        settings: Some(pb::NotificationSettings {
            server_id: on_b.id.clone(),
            channel_id: channel_id.into(),
            muted: true,
            ..Default::default()
        }),
        update_mask: Some(prost_types::FieldMask { paths: vec!["muted".into()] }),
    };
    c.account.update_notification_settings(authed(&mika, change(&channel_b.id))).await.unwrap();
    let nowhere = c.account.update_notification_settings(authed(&mika, change(&channel_a.id))).await;
    assert_eq!(nowhere.unwrap_err().code(), Code::NotFound);

    // Admins see every server, from every shard.
    let usage = c.admin.get_node_usage(authed(&juan, pb::GetNodeUsageRequest {})).await.unwrap().into_inner();
    assert_eq!((usage.accounts, usage.servers, usage.server_usage.len()), (3, 4, 4));
    let listed = c.admin.list_instance_servers(authed(ADMIN_TOKEN, pb::ListInstanceServersRequest {})).await.unwrap();
    let listed = listed.into_inner().servers;
    assert_eq!(listed.len(), 4);
    assert!(listed.iter().all(|s| s.owner.as_ref().unwrap().id == juan_user.id));
    let limits = pb::SetServerLimitsRequest {
        server_id: on_b.id.clone(),
        limits: Some(pb::ServerLimits { channels: Some(5), ..Default::default() }),
    };
    let set = c.admin.set_server_limits(authed(&juan, limits)).await.unwrap().into_inner();
    assert_eq!(set.limits.unwrap().channels, Some(5));

    // Settings changed on the directory reach the shards.
    let settings = pb::InstanceSettings {
        default_limits: Some(pb::ServerLimits { members: Some(2), ..Default::default() }),
        ..Default::default()
    };
    let update = pb::UpdateSettingsRequest {
        settings: Some(settings),
        update_mask: Some(prost_types::FieldMask { paths: vec!["default_limits.members".into()] }),
        reset_mask: None,
    };
    c.admin.update_settings(authed(&juan, update)).await.unwrap();
    let (kai, _) = sign_up(&mut c, "kai").await;
    let mut full = None;
    for _ in 0..50 {
        match join(&mut c, &kai, &on_a.id).await {
            Err(status) if status.code() == Code::ResourceExhausted => {
                full = Some(status);
                break;
            }
            Err(status) => panic!("{status:?}"),
            Ok(_) => {
                let leave = pb::LeaveServerRequest { server_id: on_a.id.clone() };
                c.servers.leave_server(authed(&kai, leave)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    assert!(full.unwrap().message().contains("full"));

    // Pictures go up and come back through the gateway, and a shard's server
    // can use one as its icon.
    let png = {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(600, 7);
        bytes
    };
    let reserved = c
        .media
        .create_upload(authed(
            &juan,
            pb::CreateUploadRequest {
                purpose: pb::MediaPurpose::ServerIcon as i32,
                content_type: "image/png".into(),
                size: png.len() as i64,
            },
        ))
        .await
        .unwrap()
        .into_inner();
    let http = reqwest::Client::new();
    // The gateway holds no more of a call than any part would take, and
    // answers as soon as it has read too much. Over HTTP/2, as gRPC goes:
    // over HTTP/1.1 the call is turned away (400, no grpc-status) before the
    // gateway reads any of it.
    let mut grpc = tonic::transport::Channel::from_shared(cluster.gateway.url()).unwrap().connect().await.unwrap();
    let huge = http::Request::post(format!("{}/fuwa.v1.NodeService/GetNode", cluster.gateway.url()))
        .header("content-type", "application/grpc")
        .body(tonic::body::Body::new("\0".repeat(9 * 1024 * 1024)))
        .unwrap();
    std::future::poll_fn(|cx| tonic::codegen::Service::poll_ready(&mut grpc, cx)).await.unwrap();
    let huge = tonic::codegen::Service::call(&mut grpc, huge).await.unwrap();
    assert_eq!(huge.headers()["grpc-status"], (Code::ResourceExhausted as i32).to_string().as_str());
    assert!(reserved.upload_url.starts_with(&cluster.gateway.url()));
    let put = http.put(&reserved.upload_url).body(png.clone()).send().await.unwrap();
    assert_eq!(put.status(), reqwest::StatusCode::NO_CONTENT);
    let picture = reserved.media.unwrap();
    let fetched = http.get(&picture.url).send().await.unwrap();
    assert_eq!(fetched.status(), 200);
    assert_eq!(fetched.bytes().await.unwrap().to_vec(), png);
    let icon = pb::UpdateServerRequest {
        server_id: on_b.id.clone(),
        icon_url: Some(picture.url.clone()),
        ..Default::default()
    };
    c.servers.update_server(authed(&juan, icon)).await.unwrap();
    let row = cluster.directory.app().node().unwrap().media(&picture.id).await.unwrap().unwrap();
    assert!(row.used);
    assert_eq!(row.server_id.as_deref(), Some(on_b.id.as_str()));
    // The server's shard keeps its pictures: the directory lets go of it once
    // the shard has it, and its link leads there.
    let on_shard = root.path().join("shard-b").join("server-pictures").join(&on_b.id).join(&picture.id);
    let at_directory = root.path().join("directory").join("media").join(&picture.id);
    wait_for(|| on_shard.exists() && !at_directory.exists()).await;
    assert_eq!(std::fs::read(&on_shard).unwrap(), png);
    let fetched = http.get(&picture.url).send().await.unwrap();
    assert_eq!(fetched.status(), 200);
    assert_eq!(fetched.headers()["content-type"], "image/png");
    assert_eq!(fetched.headers()["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(fetched.bytes().await.unwrap().to_vec(), png);
    let plain = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let redirected = plain.get(&picture.url).send().await.unwrap();
    assert_eq!(redirected.status(), 308);
    assert_eq!(redirected.headers()["location"], format!("/media/servers/{}/{}", on_b.id, picture.id).as_str());
    let elsewhere = format!("{}/media/servers/{}/{}", cluster.gateway.url(), on_a.id, picture.id);
    assert_eq!(http.get(&elsewhere).send().await.unwrap().status(), 404, "only under its own server");
    let theirs = pb::UpdateServerRequest {
        server_id: on_a.id.clone(),
        icon_url: Some(picture.url.clone()),
        ..Default::default()
    };
    assert_eq!(c.servers.update_server(authed(&mika, theirs)).await.unwrap_err().code(), Code::PermissionDenied);

    // Browsers reach everything through the gateway too.
    let response = http
        .post(format!("{}/fuwa.v1.ServerService/GetServer", cluster.gateway.url()))
        .header("content-type", "application/grpc-web+proto")
        .header("authorization", format!("Bearer {mika}"))
        .header("origin", "https://fuwa.waifu.dev")
        .body({
            let payload = prost::Message::encode_to_vec(&pb::GetServerRequest { server_id: on_b.id.clone() });
            let mut framed = vec![0u8];
            framed.extend((payload.len() as u32).to_be_bytes());
            framed.extend(payload);
            framed
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-origin"], "https://fuwa.waifu.dev");
    let body = response.bytes().await.unwrap();
    let length = u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize;
    let reply = <pb::GetServerResponse as prost::Message>::decode(&body[5..5 + length]).unwrap();
    assert_eq!(reply.server.unwrap().icon_url, picture.url);
    assert_eq!(
        http.get(format!("{}/healthz", cluster.gateway.url())).send().await.unwrap().text().await.unwrap(),
        "ok"
    );

    // The parts behind the gateway answer only calls carrying the cluster key.
    let mut direct = clients(&cluster.shards[0]).await;
    let refused = direct.servers.get_server(authed(&mika, pb::GetServerRequest { server_id: on_a.id.clone() })).await;
    assert_eq!(refused.unwrap_err().code(), Code::PermissionDenied);
    let mut direct = clients(&cluster.directory).await;
    assert_eq!(direct.node.get_node(pb::GetNodeRequest {}).await.unwrap_err().code(), Code::PermissionDenied);
    assert_eq!(http.get(format!("{}/healthz", cluster.directory.url())).send().await.unwrap().status(), 200);

    // Webhook posts reach the shard holding their server through a gateway.
    let channel = general(&mut c, &juan, &on_b.id).await;
    let request = pb::CreateWebhookRequest {
        server_id: on_b.id.clone(),
        channel_id: channel.id.clone(),
        name: "CI".into(),
        avatar_url: String::new(),
    };
    let hook = c.webhooks.create_webhook(authed(&juan, request)).await.unwrap().into_inner().webhook.unwrap();
    let url = format!("{}/webhooks/{}/{}/{}?wait=true", cluster.gateway.url(), on_b.id, hook.id, hook.token);
    let posted = http.post(&url).header("content-type", "application/json").body(r#"{"content":"green"}"#);
    let posted = posted.send().await.unwrap();
    assert_eq!(posted.status(), 200);
    let posted: serde_json::Value = posted.json().await.unwrap();
    assert_eq!(posted["content"], "green");
    let wrong = format!("{}/webhooks/{}/{}/{}", cluster.gateway.url(), on_b.id, hook.id, "x".repeat(64));
    assert_eq!(http.post(&wrong).body("{}").send().await.unwrap().status(), 404);

    // Identity providers come back to the shard holding a server (or the
    // directory, for the instance's own sign-in) through a gateway.
    let metadata = format!("{}/sso/servers/{}/saml/metadata", cluster.gateway.url(), on_b.id);
    let metadata = http.get(&metadata).send().await.unwrap();
    assert_eq!(metadata.status(), 200);
    assert!(metadata.text().await.unwrap().contains(&format!("/sso/servers/{}/saml", on_b.id)));
    let instance = http.get(format!("{}/sso/instance/saml/metadata", cluster.gateway.url())).send().await.unwrap();
    assert_eq!(instance.status(), 200);
    let stray =
        http.get(format!("{}/sso/servers/{}/oidc?state=nope", cluster.gateway.url(), on_b.id)).send().await.unwrap();
    assert_eq!(stray.status(), 400, "a sign-in nobody started");

    // Agents are made on the directory and added on the shard holding the
    // server, which asks the directory for them.
    let request = pb::CreateAgentRequest { username: "relay".into(), display_name: "Relay".into() };
    let made = c.agents.create_agent(authed(&juan, request)).await.unwrap().into_inner();
    let reset = pb::UpdateSettingsRequest {
        settings: None,
        update_mask: None,
        reset_mask: Some(prost_types::FieldMask { paths: vec!["default_limits.members".into()] }),
    };
    c.admin.update_settings(authed(&juan, reset)).await.unwrap();
    let add = pb::AddAgentRequest { server_id: on_b.id.clone(), username: "relay".into() };
    // The shard hears about the reset shortly.
    let mut member = None;
    for _ in 0..50 {
        match c.agents.add_agent(authed(&juan, add.clone())).await {
            Ok(added) => {
                member = added.into_inner().member;
                break;
            }
            Err(status) if status.code() == Code::ResourceExhausted => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(status) => panic!("{status:?}"),
        }
    }
    assert_eq!(member.unwrap().user.unwrap().kind, pb::AccountKind::Agent as i32);
    let said = pb::SendMessageRequest {
        server_id: on_b.id.clone(),
        channel_id: channel.id.clone(),
        content: "relayed".into(),
        ..Default::default()
    };
    c.messages.send_message(authed(&made.token, said)).await.unwrap();
    let mine = c.agents.list_agents(authed(&juan, pb::ListAgentsRequest {})).await.unwrap().into_inner().agents;
    assert_eq!(mine[0].servers, 1);

    // A moderation provider set up on the directory reaches the shards, key
    // and all (they check messages), while clients only learn a key is set.
    let jev = pb::AutoModProviderSettings {
        id: "typesafe-jev".into(),
        enabled: true,
        api_key: "made-up-key-for-a-test".into(),
        ..Default::default()
    };
    let update = pb::UpdateSettingsRequest {
        settings: Some(pb::InstanceSettings { automod_providers: vec![jev], ..Default::default() }),
        update_mask: Some(prost_types::FieldMask { paths: vec!["automod_providers".into()] }),
        reset_mask: None,
    };
    let saved = c.admin.update_settings(authed(&juan, update)).await.unwrap().into_inner();
    let shown = saved.config.unwrap().settings.unwrap().automod_providers;
    assert!(shown.iter().all(|p| p.api_key.is_empty()));
    let mut offered = Vec::new();
    for _ in 0..50 {
        let list = pb::ListAutoModRulesRequest { server_id: on_b.id.clone() };
        offered = c.automod.list_auto_mod_rules(authed(&juan, list)).await.unwrap().into_inner().providers;
        if !offered.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(offered[0].id, "typesafe-jev");

    // Deleting a server: the stream says so, and the directory forgets it.
    c.servers.delete_server(authed(&juan, pb::DeleteServerRequest { server_id: on_a.id.clone() })).await.unwrap();
    let deleted = until(&mut stream, |e| matches!(e.payload, Some(Payload::ServerDeleted(_)))).await;
    assert_eq!(deleted.server_id, on_a.id);
    assert_eq!(cluster.placement(&on_a.id), None);
    let gone = c.servers.get_server(authed(&juan, pb::GetServerRequest { server_id: on_a.id.clone() })).await;
    assert_eq!(gone.unwrap_err().code(), Code::NotFound);
    let mine = c.servers.list_servers(authed(&mika, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(mine.len(), 1);

    // A data export gathers every shard's part.
    let mut chunks = c.account.export_data(authed(&mika, pb::ExportDataRequest {})).await.unwrap().into_inner();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        bytes.extend(chunk.unwrap().chunk);
    }
    let export: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let exported = export["servers"].as_array().unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0]["messages"][0]["content"], "mika was here");

    // Deleting an account takes it out of every shard's servers.
    drop(stream);
    let delete = pb::DeleteAccountRequest { password: PASSWORD.into(), ..Default::default() };
    c.account.delete_account(authed(&mika, delete)).await.unwrap();
    let left = members(&mut c, &juan, &on_b.id).await;
    assert!(left.iter().all(|m| m.user.as_ref().unwrap().id != mika_user.id));
    assert_eq!(c.auth.get_me(authed(&mika, pb::GetMeRequest {})).await.unwrap_err().code(), Code::Unauthenticated);

    cluster.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shards_come_and_go() {
    let root = tempfile::tempdir().unwrap();
    let mut cluster = start_cluster(root.path(), &[]).await;
    let mut c = clients(&cluster.gateway).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let first = create_server(&mut c, &juan, "First").await;
    let second = create_server(&mut c, &juan, "Second").await;
    let on_b = if cluster.placement(&first.id).as_deref() == Some("b") { first } else { second };
    assert_eq!(cluster.placement(&on_b.id).as_deref(), Some("b"));
    let channel = general(&mut c, &juan, &on_b.id).await;
    let code = invite(&mut c, &juan, &on_b.id).await.code;

    // A live stream following a shard that stops waits a while for it to come
    // back, then ends, so the client follows again.
    let cursors = vec![pb::ServerCursor { server_id: on_b.id.clone(), after_sequence: None }];
    let mut stream = c
        .events
        .subscribe(authed(&juan, pb::SubscribeRequest { servers: cursors.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert!(next(&mut stream).await.ready.is_some());

    // While shard b stays down, its servers can't be reached; the rest can.
    let b = cluster.shards.remove(1);
    b.stop().await;
    let ended = tokio::time::timeout(RIDE_OUT * 4, stream.next()).await.unwrap();
    assert_eq!(ended.unwrap().unwrap_err().code(), Code::Unavailable);
    let mut down = None;
    for _ in 0..50 {
        let request = pb::GetServerRequest { server_id: on_b.id.clone() };
        match c.servers.get_server(authed(&juan, request)).await {
            Err(status) if status.code() == Code::Unavailable => {
                down = Some(status);
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert!(down.is_some(), "a down shard's servers read as unavailable");
    assert!(c.servers.list_servers(authed(&juan, pb::ListServersRequest {})).await.is_ok());
    let made =
        c.servers.create_server(authed(&juan, pb::CreateServerRequest { name: "Third".into(), ..Default::default() }));
    let made = made.await.unwrap().into_inner().server.unwrap();
    assert_eq!(cluster.placement(&made.id).as_deref(), Some("a"), "new servers go to shards that are up");
    // Deleting an account needs every shard, so nobody's name stays behind.
    let (temp, _) = sign_up(&mut c, "temp").await;
    let delete = pb::DeleteAccountRequest { password: PASSWORD.into(), ..Default::default() };
    assert_eq!(c.account.delete_account(authed(&temp, delete)).await.unwrap_err().code(), Code::Unavailable);

    // Back on another port, it registers again and its servers work.
    let b = start_shard(&root.path().join("shard-b"), "b", &cluster.directory).await;
    cluster.shards.push(b);
    eventually(|| {
        let mut c = c.messages.clone();
        let request = pb::SendMessageRequest {
            server_id: on_b.id.clone(),
            channel_id: channel.id.clone(),
            content: "back".into(),
            ..Default::default()
        };
        let request = authed(&juan, request);
        async move { c.send_message(request).await }
    })
    .await;

    // Moving a server: stop its shard, move the file, start the other one.
    let b = cluster.shards.remove(1);
    b.stop().await;
    let a = cluster.shards.remove(0);
    a.stop().await;
    let file = format!("{}.db", on_b.id);
    for suffix in ["", "-log", "-wal", "-shm"] {
        let from = root.path().join("shard-b/servers").join(format!("{file}{suffix}"));
        if from.exists() {
            std::fs::rename(&from, root.path().join("shard-a/servers").join(format!("{file}{suffix}"))).unwrap();
        }
    }
    cluster.shards.push(start_shard(&root.path().join("shard-a"), "a", &cluster.directory).await);
    cluster.shards.push(start_shard(&root.path().join("shard-b"), "b", &cluster.directory).await);
    assert_eq!(cluster.placement(&on_b.id).as_deref(), Some("a"));
    let listed = eventually(|| {
        let mut c = c.messages.clone();
        let request = pb::ListMessagesRequest {
            server_id: on_b.id.clone(),
            channel_id: channel.id.clone(),
            ..Default::default()
        };
        let request = authed(&juan, request);
        async move { c.list_messages(request).await }
    })
    .await;
    assert!(listed.into_inner().messages.iter().any(|m| m.content == "back"));
    // Its invites follow it to the shard that holds it now.
    let shown = c.invites.get_invite(pb::GetInviteRequest { code }).await.unwrap().into_inner();
    assert_eq!(shown.server.unwrap().id, on_b.id);

    cluster.stop().await;
}

/// A deploy restarts each part. Shards and the directory keep their files on
/// a volume, so the old process stops before the new one starts: the gateway
/// holds calls and live streams over the gap, and nobody notices.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarts_go_unnoticed() {
    let root = tempfile::tempdir().unwrap();
    let mut cluster = start_cluster(root.path(), &[]).await;
    let mut c = clients(&cluster.gateway).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let mut on_b = None;
    while on_b.is_none() {
        let server = create_server(&mut c, &juan, "Server").await;
        if cluster.placement(&server.id).as_deref() == Some("b") {
            on_b = Some(server);
        }
    }
    let on_b = on_b.unwrap();
    let channel = general(&mut c, &juan, &on_b.id).await;
    let cursors = vec![pb::ServerCursor { server_id: on_b.id.clone(), after_sequence: None }];
    let mut stream =
        c.events.subscribe(authed(&juan, pb::SubscribeRequest { servers: cursors })).await.unwrap().into_inner();
    assert!(next(&mut stream).await.ready.is_some());

    // Shard b restarts: a message sent meanwhile goes through once it's back,
    // and the stream, which never ends, brings it.
    let b = cluster.shards.remove(1);
    b.stop().await;
    let sending = {
        let (mut c, juan, server_id, channel_id) =
            (c.messages.clone(), juan.clone(), on_b.id.clone(), channel.id.clone());
        tokio::spawn(async move {
            let request =
                pb::SendMessageRequest { server_id, channel_id, content: "during".into(), ..Default::default() };
            c.send_message(authed(&juan, request)).await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    cluster.shards.push(start_shard(&root.path().join("shard-b"), "b", &cluster.directory).await);
    sending.await.unwrap().expect("a call waits for its shard to come back");
    let event = until(&mut stream, |e| matches!(&e.payload, Some(Payload::MessageCreated(_)))).await;
    let Some(Payload::MessageCreated(created)) = event.payload else { unreachable!() };
    assert_eq!(created.message.unwrap().content, "during");

    // The directory restarts on its address: calls it answers, and calls a
    // shard needs it for (whose sign-in check it no longer remembers), wait
    // for it too.
    let addr = cluster.directory.addr;
    let gateway_addr = cluster.gateway.addr;
    let directory = std::mem::replace(&mut cluster.directory, stopped(addr));
    directory.stop().await;
    // Long enough for shard b to forget who juan is.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let listing = {
        let (mut c, juan) = (c.servers.clone(), juan.clone());
        tokio::spawn(async move { c.list_servers(authed(&juan, pb::ListServersRequest {})).await })
    };
    let sending = {
        let (mut c, juan, server_id, channel_id) =
            (c.messages.clone(), juan.clone(), on_b.id.clone(), channel.id.clone());
        tokio::spawn(async move {
            let request =
                pb::SendMessageRequest { server_id, channel_id, content: "after".into(), ..Default::default() };
            c.send_message(authed(&juan, request)).await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let listener = TcpListener::bind(addr).await.unwrap();
    cluster.directory = start_directory(root.path(), &[], listener, addr, gateway_addr).await;
    let listed = listing.await.unwrap().expect("a call waits for the directory to come back");
    assert!(listed.into_inner().servers.iter().any(|s| s.id == on_b.id));
    sending.await.unwrap().expect("a shard waits for the directory to check who's calling");
    let event = until(&mut stream, |e| matches!(&e.payload, Some(Payload::MessageCreated(_)))).await;
    let Some(Payload::MessageCreated(created)) = event.payload else { unreachable!() };
    assert_eq!(created.message.unwrap().content, "after");

    cluster.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_process_splits_in_place() {
    let root = tempfile::tempdir().unwrap();
    let one = root.path().join("one");
    // Encrypted, as fuwa.chat is: the files move as they are.
    let encrypted = [("FUWA_ENCRYPTION_KEY", "ab".repeat(32))];

    // An instance that ran as one process, with servers and messages.
    let (listener, addr) = listen().await;
    let single = start_app(config(&one, &encrypted), listener, addr).await;
    let mut c = clients(&single).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let first = create_server(&mut c, &juan, "First").await;
    let second = create_server(&mut c, &juan, "Second").await;
    let channel = general(&mut c, &juan, &first.id).await;
    send(&mut c, &juan, &first.id, &channel.id, "from before the split").await;
    drop(c);
    single.stop().await;

    // Its folder becomes the directory's, and the first shard to start takes
    // the servers; the next gets none of them.
    let (gateway_listener, gateway_addr) = listen().await;
    let (listener, addr) = listen().await;
    let directory_vars = [
        ("FUWA_ROLE", "directory".to_string()),
        ("FUWA_PUBLIC_URL", format!("http://{gateway_addr}")),
        encrypted[0].clone(),
    ];
    let directory = start_app(config(&one, &directory_vars), listener, addr).await;
    let a = start_shard_with(&root.path().join("shard-a"), "a", &directory, &encrypted).await;
    let b = start_shard_with(&root.path().join("shard-b"), "b", &directory, &encrypted).await;
    for server in [&first, &second] {
        assert!(a.app().servers.holds(&server.id) && !b.app().servers.holds(&server.id));
        assert_eq!(directory.app().index.placement(&server.id).as_deref(), Some("a"));
        let file = format!("{}.db", server.id);
        assert!(!one.join("servers").join(&file).exists());
        assert!(one.join("handed-over").join(&file).exists(), "the directory keeps its copy out of the way");
    }
    assert!(!one.join("servers-promised-to").exists());
    assert!(!root.path().join("shard-a/incoming-servers").exists());

    // Through a gateway it's the same instance: the same session, servers and messages.
    let (gateway, _gateway) = start_gateway(&root.path().join("gateway"), &directory, gateway_listener, gateway_addr);
    let mut c = clients(&gateway).await;
    let mine = c.servers.list_servers(authed(&juan, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    assert_eq!(
        mine.iter().map(|s| s.id.as_str()).collect::<HashSet<_>>(),
        HashSet::from([first.id.as_str(), second.id.as_str()])
    );
    let request =
        pb::ListMessagesRequest { server_id: first.id.clone(), channel_id: channel.id.clone(), ..Default::default() };
    let listed = c.messages.list_messages(authed(&juan, request)).await.unwrap().into_inner();
    assert!(listed.messages.iter().any(|m| m.content == "from before the split"));
    send(&mut c, &juan, &first.id, &channel.id, "after").await;

    // Starting again, the shard has nothing more to take and keeps what it has.
    a.stop().await;
    let a = start_shard_with(&root.path().join("shard-a"), "a", &directory, &encrypted).await;
    assert!(a.app().servers.holds(&first.id) && a.app().servers.holds(&second.id));

    gateway.stop().await;
    a.stop().await;
    b.stop().await;
    directory.stop().await;
}

mod common;

/// A media part on `addr` (its API) carrying calls on a port of its own.
async fn start_media(dir: &Path, listener: TcpListener, addr: SocketAddr) -> (Part, CancellationToken) {
    let vars = [
        ("FUWA_ROLE", "media".to_string()),
        ("FUWA_MEDIA_PORT", "0".to_string()),
        ("FUWA_MEDIA_ADDRESSES", "127.0.0.1".to_string()),
    ];
    let calls = CancellationToken::new();
    let (router, _) = fuwa_server::cluster::media::start(&config(dir, &vars), calls.clone()).await.unwrap();
    let shutdown = CancellationToken::new();
    let serving = serve(listener, router, shutdown.clone());
    (Part { app: None, addr, shutdown, serving }, calls)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_ride_out_a_media_restart() {
    let root = tempfile::tempdir().unwrap();
    let (media_listener, media_addr) = listen().await;
    let (media, calls) = start_media(&root.path().join("media"), media_listener, media_addr).await;
    let media_url = [("FUWA_MEDIA_URL", media.url())];
    let (gateway_listener, gateway_addr) = listen().await;
    let (listener, addr) = listen().await;
    let directory = start_directory(root.path(), &media_url, listener, addr, gateway_addr).await;
    let shard = start_shard_with(&root.path().join("shard-a"), "a", &directory, &media_url).await;
    let (gateway, _gateway) = start_gateway(&root.path().join("gateway"), &directory, gateway_listener, gateway_addr);

    let channel = Channel::from_shared(gateway.url()).unwrap().connect().await.unwrap();
    let mut c = clients(&gateway).await;
    let mut voice = pb::call_service_client::CallServiceClient::new(channel);
    let (juan, _) = sign_up(&mut c, "juan").await;
    let (mika, _) = sign_up(&mut c, "mika").await;
    let (helper, _) = sign_up(&mut c, "helper").await;
    let settings = voice.get_call_settings(authed(&juan, pb::GetCallSettingsRequest {})).await.unwrap().into_inner();
    assert!(settings.enabled, "the directory knows where the media part is");
    let server = create_server(&mut c, &juan, "Calls").await;
    join(&mut c, &mika, &server.id).await.unwrap();
    join(&mut c, &helper, &server.id).await.unwrap();
    let lounge = c
        .channels
        .create_channel(authed(
            &juan,
            pb::CreateChannelRequest {
                server_id: server.id.clone(),
                name: "Lounge".into(),
                r#type: pb::ChannelType::Voice as i32,
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .channel
        .unwrap();

    let join_voice = |token: String, offer: String, session_id: String| {
        let mut voice = voice.clone();
        let request = pb::JoinVoiceRequest {
            server_id: server.id.clone(),
            channel_id: lounge.id.clone(),
            offer,
            session_id,
            ..Default::default()
        };
        async move { voice.join_voice(authed(&token, request)).await.unwrap().into_inner() }
    };
    let (mut a, offer) = common::Peer::new().await;
    let a_joined = join_voice(juan.clone(), offer, String::new()).await;
    a.answer(&a_joined.answer);
    let (mut b, offer) = common::Peer::new().await;
    let b_joined = join_voice(mika.clone(), offer, String::new()).await;
    b.answer(&b_joined.answer);
    // A program listens too, through the gateway, the shard and the media part.
    let mut listening = voice
        .clone()
        .listen_voice(authed(
            &helper,
            pb::ListenVoiceRequest {
                server_id: server.id.clone(),
                channel_id: lounge.id.clone(),
                ..Default::default()
            },
        ))
        .await
        .unwrap()
        .into_inner();
    listening.next().await.unwrap().unwrap();
    let heard = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting = heard.clone();
    let program = tokio::spawn(async move {
        while let Some(Ok(message)) = listening.next().await {
            if matches!(message.event, Some(pb::listen_voice_response::Event::Frame(_))) {
                counting.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
    common::talk(&mut a, &mut b, Duration::from_secs(3)).await;
    assert!(b.heard.len() > 20, "sound goes through the split instance's media part");
    let before = heard.load(std::sync::atomic::Ordering::Relaxed);
    assert!(before > 40, "the program heard {before} frames");

    // A deploy: the media part tells everyone, and a new one takes its place.
    calls.cancel();
    common::talk(&mut a, &mut b, Duration::from_millis(500)).await;
    assert!(a.signals.iter().any(|s| s == "restarting") && b.signals.iter().any(|s| s == "restarting"));
    media.stop().await;
    let (media_listener, _) = (TcpListener::bind(media_addr).await.unwrap(), ());
    let (media, _calls) = start_media(&root.path().join("media"), media_listener, media_addr).await;

    // Both join again with the sessions they had: nobody left the call.
    let (mut a, offer) = common::Peer::new().await;
    let again = join_voice(juan.clone(), offer, a_joined.session_id.clone()).await;
    assert_eq!(again.session_id, a_joined.session_id);
    a.answer(&again.answer);
    let (mut b, offer) = common::Peer::new().await;
    b.answer(&join_voice(mika.clone(), offer, b_joined.session_id.clone()).await.answer);
    common::talk(&mut a, &mut b, Duration::from_secs(3)).await;
    assert!(b.heard.len() > 20, "sound is back after the restart");
    let after = heard.load(std::sync::atomic::Ordering::Relaxed) - before;
    assert!(after > 40, "the program heard {after} frames after the restart, without listening again");
    assert!(!program.is_finished(), "its stream stayed open");
    let states = voice
        .list_voice_states(authed(&juan, pb::ListVoiceStatesRequest { server_id: server.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .states;
    assert_eq!(states.len(), 3);

    program.abort();
    media.stop().await;
    gateway.stop().await;
    shard.stop().await;
    directory.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn channels_are_shared_across_shards() {
    let root = tempfile::tempdir().unwrap();
    let cluster = start_cluster(root.path(), &[]).await;
    let mut c = clients(&cluster.gateway).await;
    let (juan, _) = sign_up(&mut c, "juan").await;
    let (mika, _) = sign_up(&mut c, "mika").await;
    let (rin, _) = sign_up(&mut c, "rin").await;
    let home = create_server(&mut c, &juan, "Home").await;
    let guest = create_server(&mut c, &mika, "Guest").await;
    assert_ne!(cluster.placement(&home.id), cluster.placement(&guest.id), "each on its own shard");
    join(&mut c, &rin, &guest.id).await.unwrap();
    let dev = general(&mut c, &juan, &home.id).await;

    let code = c
        .shared
        .create_share_code(authed(
            &juan,
            pb::CreateShareCodeRequest { server_id: home.id.clone(), channel_id: dev.id.clone() },
        ))
        .await
        .unwrap()
        .into_inner()
        .code
        .unwrap()
        .code;
    let preview = c
        .shared
        .preview_share(authed(&mika, pb::PreviewShareRequest { server_id: guest.id.clone(), code: code.clone() }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(preview.home_server.unwrap().name, "Home");
    let asked = c
        .shared
        .accept_share(authed(&mika, pb::AcceptShareRequest { server_id: guest.id.clone(), code, ..Default::default() }))
        .await
        .unwrap()
        .into_inner()
        .connection
        .unwrap();
    c.shared
        .review_share(authed(
            &juan,
            pb::ReviewShareRequest { server_id: home.id.clone(), connection_id: asked.id, approve: true },
        ))
        .await
        .unwrap();
    let shown = c
        .channels
        .list_channels(authed(&rin, pb::ListChannelsRequest { server_id: guest.id.clone() }))
        .await
        .unwrap()
        .into_inner()
        .channels
        .into_iter()
        .find(|ch| ch.shared.is_some())
        .unwrap();

    let mut stream = c
        .events
        .subscribe(authed(
            &rin,
            pb::SubscribeRequest {
                servers: vec![pb::ServerCursor { server_id: guest.id.clone(), after_sequence: None }],
            },
        ))
        .await
        .unwrap()
        .into_inner();
    send(&mut c, &juan, &home.id, &dev.id, "across shards").await;
    let event = until(&mut stream, |e| matches!(e.payload, Some(Payload::MessageCreated(_)))).await;
    let Some(Payload::MessageCreated(created)) = event.payload else { unreachable!() };
    let message = created.message.unwrap();
    assert_eq!((message.content.as_str(), message.channel_id.as_str()), ("across shards", shown.id.as_str()));

    send(&mut c, &rin, &guest.id, &shown.id, "and back").await;
    let at_home = c
        .messages
        .list_messages(authed(
            &juan,
            pb::ListMessagesRequest { server_id: home.id.clone(), channel_id: dev.id.clone(), ..Default::default() },
        ))
        .await
        .unwrap()
        .into_inner()
        .messages;
    let back = at_home.iter().find(|m| m.content == "and back").unwrap();
    assert_eq!(back.shared.as_ref().unwrap().server.as_ref().unwrap().name, "Guest");
    cluster.stop().await;
}

/// Servers live in the region their creator picked, and an admin can move one
/// to another region: changes pause for a moment, live streams carry on, and
/// nothing of it is left where it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn servers_live_in_their_region_and_move() {
    let root = tempfile::tempdir().unwrap();
    let (gateway_listener, gateway_addr) = listen().await;
    let (listener, addr) = listen().await;
    let home = [("FUWA_REGION", "us-east".to_string())];
    let directory = start_directory(root.path(), &home, listener, addr, gateway_addr).await;
    // Shard a doesn't say where it is: the home region. Shard b is in Europe,
    // replicating to a bucket of its own.
    let bucket = |name: &str| ("FUWA_REPLICA_PATH", root.path().join(name).to_str().unwrap().to_string());
    let a = start_shard_with(&root.path().join("shard-a"), "a", &directory, &[bucket("bucket-us")]).await;
    let eu = [("FUWA_REGION", "eu".to_string()), bucket("bucket-eu")];
    let b = start_shard_with(&root.path().join("shard-b"), "b", &directory, &eu).await;
    for shard in [&a, &b] {
        shard.app().replica.as_ref().unwrap().start();
    }
    let (gateway, _gateway) = start_gateway(&root.path().join("gateway"), &directory, gateway_listener, gateway_addr);
    let cluster = Cluster { directory, shards: vec![a, b], gateway, _gateway };
    let mut c = clients(&cluster.gateway).await;

    let node = c.node.get_node(pb::GetNodeRequest {}).await.unwrap().into_inner().node.unwrap();
    let regions: Vec<(&str, &str, bool)> =
        node.regions.iter().map(|r| (r.id.as_str(), r.name.as_str(), r.home)).collect();
    assert_eq!(regions, [("us-east", "US East", true), ("eu", "Europe", false)]);

    let (juan, _) = sign_up(&mut c, "juan").await;
    let make = |region: &str| {
        let request = pb::CreateServerRequest { name: "Somewhere".into(), region: region.into(), ..Default::default() };
        let mut servers = c.servers.clone();
        let request = authed(&juan, request);
        async move { servers.create_server(request).await.map(|r| r.into_inner().server.unwrap()) }
    };
    // New servers go to their region's shards.
    let in_eu = make("eu").await.unwrap();
    assert_eq!((cluster.placement(&in_eu.id).as_deref(), in_eu.region.as_str()), (Some("b"), "eu"));
    for region in ["", "us-east"] {
        let at_home = make(region).await.unwrap();
        assert_eq!((cluster.placement(&at_home.id).as_deref(), at_home.region.as_str()), (Some("a"), ""));
    }
    assert_eq!(make("mars").await.unwrap_err().code(), Code::InvalidArgument);
    assert_eq!(make("Not A Region").await.unwrap_err().code(), Code::InvalidArgument);

    // A server at home, with a message, a recording's files, pictures and a
    // live stream.
    let server = make("").await.unwrap();
    let http = reqwest::Client::new();
    let upload = |purpose: pb::MediaPurpose, fill: u8| {
        let (mut media, http, juan) = (c.media.clone(), http.clone(), juan.clone());
        async move {
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.resize(300, fill);
            let request =
                pb::CreateUploadRequest { purpose: purpose as i32, content_type: "image/png".into(), size: 300 };
            let reserved = media.create_upload(authed(&juan, request)).await.unwrap().into_inner();
            assert_eq!(http.put(&reserved.upload_url).body(png.clone()).send().await.unwrap().status(), 204);
            (reserved.media.unwrap(), png)
        }
    };
    let (icon, icon_png) = upload(pb::MediaPurpose::ServerIcon, 1).await;
    let request = pb::UpdateServerRequest {
        server_id: server.id.clone(),
        icon_url: Some(icon.url.clone()),
        ..Default::default()
    };
    c.servers.update_server(authed(&juan, request)).await.unwrap();
    let shard_a = root.path().join("shard-a");
    let picture_at = |dir: &Path, id: &str| dir.join("server-pictures").join(&server.id).join(id);
    wait_for(|| picture_at(&shard_a, &icon.id).exists()).await;
    wait_for(|| picture_at(&root.path().join("bucket-us"), &icon.id).exists()).await;
    // An emoji it used before servers' shards kept their pictures: still at
    // the directory, and taken when the server moves.
    let (emoji, emoji_png) = upload(pb::MediaPurpose::Emoji, 2).await;
    cluster.directory.app().node().unwrap().use_media(&emoji.id, Some(&server.id)).await.unwrap();
    let channel = general(&mut c, &juan, &server.id).await;
    send(&mut c, &juan, &server.id, &channel.id, "before").await;
    let recording = format!("recordings/{}/01J9Z3K8X2V5W7Q4R6T8Y0B2C5", server.id);
    let track = "01J9Z3K8X2V5W7Q4R6T8Y0B2C6.opus";
    std::fs::create_dir_all(shard_a.join(&recording)).unwrap();
    std::fs::write(shard_a.join(&recording).join(track), b"OggS").unwrap();
    let cursors = vec![pb::ServerCursor { server_id: server.id.clone(), after_sequence: None }];
    let mut stream =
        c.events.subscribe(authed(&juan, pb::SubscribeRequest { servers: cursors })).await.unwrap().into_inner();
    assert!(next(&mut stream).await.ready.is_some());
    // The home shard's replica has it.
    let replicated = |bucket: &str| root.path().join(bucket).join("servers").join(&server.id).exists();
    for _ in 0..50 {
        if replicated("bucket-us") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(replicated("bucket-us"));

    // A picture its shard lost still goes with it, from the bucket.
    std::fs::remove_file(picture_at(&shard_a, &icon.id)).unwrap();

    // Only admins move servers (the first account is one).
    let (mika, _) = sign_up(&mut c, "mika").await;
    let request = pb::MoveServerRequest { server_id: server.id.clone(), region: "eu".into() };
    assert_eq!(c.admin.move_server(authed(&mika, request.clone())).await.unwrap_err().code(), Code::PermissionDenied);
    let moved = c.admin.move_server(authed(ADMIN_TOKEN, request.clone())).await.unwrap().into_inner();
    assert_eq!(moved.server.unwrap().region, "eu");
    assert_eq!(cluster.placement(&server.id).as_deref(), Some("b"));
    assert!(cluster.shards[1].app().servers.holds(&server.id) && !cluster.shards[0].app().servers.holds(&server.id));
    assert_eq!(
        c.admin.move_server(authed(ADMIN_TOKEN, request)).await.unwrap_err().code(),
        Code::FailedPrecondition,
        "it's already there"
    );

    // Everything came along, and it carries on in its new region.
    let request = pb::GetServerRequest { server_id: server.id.clone() };
    let got = c.servers.get_server(authed(&juan, request)).await.unwrap().into_inner();
    assert_eq!(got.server.unwrap().region, "eu");
    let shard_b = root.path().join("shard-b");
    assert!(shard_b.join(&recording).join(track).exists());
    send(&mut c, &juan, &server.id, &channel.id, "after").await;
    let request =
        pb::ListMessagesRequest { server_id: server.id.clone(), channel_id: channel.id.clone(), ..Default::default() };
    let listed = c.messages.list_messages(authed(&juan, request)).await.unwrap().into_inner().messages;
    assert_eq!(listed.iter().map(|m| m.content.as_str()).collect::<HashSet<_>>(), HashSet::from(["before", "after"]));
    // The live stream followed it to its new shard.
    let event = until(&mut stream, |e| matches!(&e.payload, Some(Payload::MessageCreated(_)))).await;
    let Some(Payload::MessageCreated(created)) = event.payload else { unreachable!() };
    assert_eq!(created.message.unwrap().content, "after");

    // Nothing is left in the old region: not on its shard, nor in its bucket.
    assert!(!shard_a.join("servers").join(format!("{}.db", server.id)).exists());
    assert!(!shard_a.join(&recording).exists());
    for _ in 0..50 {
        if replicated("bucket-eu") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(replicated("bucket-eu"), "the new region's bucket has it");
    assert!(!replicated("bucket-us"), "the old region's bucket doesn't");
    assert!(root.path().join("bucket-eu").join(&recording).join(track).exists());
    assert!(!root.path().join("bucket-us").join(&recording).exists());

    // Its pictures came along too, the emoji from the directory; their links
    // still work, and lead to the new region.
    let (bucket_eu, bucket_us) = (root.path().join("bucket-eu"), root.path().join("bucket-us"));
    let directory_media = root.path().join("directory").join("media");
    wait_for(|| picture_at(&shard_b, &emoji.id).exists() && !directory_media.join(&emoji.id).exists()).await;
    for (picture, png) in [(&icon, &icon_png), (&emoji, &emoji_png)] {
        assert_eq!(&std::fs::read(picture_at(&shard_b, &picture.id)).unwrap(), png);
        assert!(picture_at(&bucket_eu, &picture.id).exists());
        assert!(!picture_at(&shard_a, &picture.id).exists() && !picture_at(&bucket_us, &picture.id).exists());
        assert!(!directory_media.join(&picture.id).exists());
        let fetched = http.get(&picture.url).send().await.unwrap();
        assert_eq!(fetched.status(), 200);
        assert_eq!(&fetched.bytes().await.unwrap().to_vec(), png);
    }
    // A shard that loses a picture gets it back from its bucket.
    std::fs::remove_file(picture_at(&shard_b, &icon.id)).unwrap();
    assert_eq!(http.get(&icon.url).send().await.unwrap().bytes().await.unwrap().to_vec(), icon_png);
    // Replacing a picture deletes it, here and in the bucket.
    let request =
        pb::UpdateServerRequest { server_id: server.id.clone(), icon_url: Some(String::new()), ..Default::default() };
    c.servers.update_server(authed(&juan, request)).await.unwrap();
    assert!(!picture_at(&shard_b, &icon.id).exists() && !picture_at(&bucket_eu, &icon.id).exists());
    assert_eq!(http.get(&icon.url).send().await.unwrap().status(), 404);
    assert!(cluster.directory.app().node().unwrap().moves().await.unwrap().is_empty(), "the move is over");

    cluster.stop().await;
}
