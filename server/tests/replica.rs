//! A split instance replicating to a folder (standing in for a bucket),
//! losing its directory's and shard's volumes, and coming back from the
//! replica on fresh ones.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fuwa_server::app::App;
use fuwa_server::cluster::gateway::Gateway;
use fuwa_server::config::Config;
use fuwa_server::pb;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tonic::{Code, Request};

const CLUSTER_KEY: &str = "cluster-key-0123456789abcdef0123456789abcdef";
const ADMIN_TOKEN: &str = "test-admin-token-0123456789abcdef0123456789";
const KEY: &str = "b1bbfda4f589dc9daaf004fe21111e00dc00c98237102f5c7002a5669fc76327";
const PASSWORD: &str = "correct horse battery";

fn config(dir: &Path, vars: &[(&str, String)]) -> Config {
    let dir = dir.to_str().unwrap().to_string();
    let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    Config::from_lookup(|key| match key {
        "FUWA_DATA_PATH" => Some(dir.clone()),
        "FUWA_TELEMETRY" => Some("off".into()),
        "FUWA_ADMIN_TOKEN" => Some(ADMIN_TOKEN.into()),
        "FUWA_CLUSTER_KEY" => Some(CLUSTER_KEY.into()),
        "FUWA_ENCRYPTION_KEY" => Some(KEY.into()),
        "FUWA_REPLICA_INTERVAL" => Some("50ms".into()),
        _ => vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
    })
    .unwrap()
}

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

    /// Stops as `fuwa serve` does: the last commits go out first.
    async fn stop(self) {
        self.shutdown.cancel();
        self.serving.await.unwrap();
        if let Some(replica) = self.app.as_ref().and_then(|app| app.replica.clone()) {
            replica.close().await;
        }
    }
}

fn serve(listener: TcpListener, router: axum::Router, shutdown: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        axum::serve(listener, router).with_graceful_shutdown(async move { shutdown.cancelled().await }).await.unwrap();
    })
}

async fn listen() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

/// Serves a part as `fuwa serve` does, replica running.
async fn start_on(config: Config, listener: TcpListener, addr: SocketAddr) -> Part {
    let app = App::open(config).await.unwrap();
    app.replica.as_ref().expect("the replica is on").start();
    let shutdown = app.shutdown.clone();
    let serving = serve(listener, app.router(), shutdown.clone());
    Part { app: Some(app), addr, shutdown, serving }
}

/// A directory, one shard and a gateway; the first two replicate.
struct Cluster {
    directory: Part,
    shard: Part,
    gateway: Part,
    _gateway: Arc<Gateway>,
}

impl Cluster {
    async fn stop(self) {
        self.gateway.stop().await;
        self.shard.stop().await;
        self.directory.stop().await;
    }
}

/// Where a cluster's parts keep their files, and how they start.
struct Setup<'a> {
    root: &'a Path,
    replica: String,
    restore: bool,
}

impl Setup<'_> {
    fn vars(&self, mut vars: Vec<(&'static str, String)>) -> Vec<(&'static str, String)> {
        vars.push(("FUWA_REPLICA_PATH", self.replica.clone()));
        if self.restore {
            vars.push(("FUWA_RESTORE", "if-empty".into()));
        }
        vars
    }

    fn directory(&self, gateway: SocketAddr) -> Config {
        let vars = self.vars(vec![("FUWA_ROLE", "directory".into()), ("FUWA_PUBLIC_URL", format!("http://{gateway}"))]);
        config(&self.root.join("directory"), &vars)
    }

    fn shard(&self, directory: &str, addr: SocketAddr) -> Config {
        let vars = self.vars(vec![
            ("FUWA_ROLE", "shard".into()),
            ("FUWA_SHARD_ID", "shard-1".into()),
            ("FUWA_DIRECTORY_URL", directory.into()),
            ("FUWA_INTERNAL_URL", format!("http://{addr}")),
        ]);
        config(&self.root.join("shard"), &vars)
    }

    async fn start(&self) -> Cluster {
        let (gateway_listener, gateway_addr) = listen().await;
        let (listener, addr) = listen().await;
        let directory = start_on(self.directory(gateway_addr), listener, addr).await;
        let (listener, addr) = listen().await;
        let shard = start_on(self.shard(&directory.url(), addr), listener, addr).await;
        let vars = [("FUWA_ROLE", "gateway".to_string()), ("FUWA_DIRECTORY_URL", directory.url())];
        let gateway = Gateway::new(config(&self.root.join("gateway"), &vars)).unwrap();
        let shutdown = gateway.shutdown();
        let serving = serve(gateway_listener, gateway.router(), shutdown.clone());
        let part = Part { app: None, addr: gateway_addr, shutdown, serving };
        Cluster { directory, shard, gateway: part, _gateway: gateway }
    }

    /// Why the directory and the shard won't open. A shard first asks the
    /// directory for servers to take over, so it gets a restored one to ask.
    async fn refusals(&self) -> (String, String) {
        let unused: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let directory = App::open(self.directory(unused)).await.err().expect("the directory opened");
        assert!(!self.root.join("directory/node.db").exists());
        let restoring = Setup { root: self.root, replica: self.replica.clone(), restore: true };
        let (listener, addr) = listen().await;
        let running = start_on(restoring.directory(unused), listener, addr).await;
        let shard = App::open(self.shard(&running.url(), unused)).await.err().expect("the shard opened");
        running.stop().await;
        (directory.to_string(), shard.to_string())
    }
}

fn authed<T>(token: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

type Messages = pb::message_service_client::MessageServiceClient<Channel>;

struct Clients {
    auth: pb::auth_service_client::AuthServiceClient<Channel>,
    servers: pb::server_service_client::ServerServiceClient<Channel>,
    channels: pb::channel_service_client::ChannelServiceClient<Channel>,
    messages: Messages,
    media: pb::media_service_client::MediaServiceClient<Channel>,
    dms: pb::direct_message_service_client::DirectMessageServiceClient<Channel>,
}

async fn clients(part: &Part) -> Clients {
    let channel = Channel::from_shared(part.url()).unwrap().connect().await.unwrap();
    Clients {
        auth: pb::auth_service_client::AuthServiceClient::new(channel.clone()),
        servers: pb::server_service_client::ServerServiceClient::new(channel.clone()),
        channels: pb::channel_service_client::ChannelServiceClient::new(channel.clone()),
        messages: pb::message_service_client::MessageServiceClient::new(channel.clone()),
        media: pb::media_service_client::MediaServiceClient::new(channel.clone()),
        dms: pb::direct_message_service_client::DirectMessageServiceClient::new(channel),
    }
}

/// Signs in, waiting for parts that are still starting.
async fn sign_in(c: &mut Clients) -> String {
    for _ in 0..100 {
        let request = pb::SignInRequest { username: "juan".into(), password: PASSWORD.into() };
        match c.auth.sign_in(request).await {
            Ok(response) => return response.into_inner().token,
            Err(status) if status.code() == Code::Unavailable => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(status) => panic!("{status:?}"),
        }
    }
    panic!("the directory never came up");
}

async fn create_server(c: &mut Clients, token: &str, name: &str) -> (String, String) {
    let request = pb::CreateServerRequest { name: name.into(), ..Default::default() };
    let server = c.servers.create_server(authed(token, request)).await.unwrap().into_inner().server.unwrap();
    let request = pb::ListChannelsRequest { server_id: server.id.clone() };
    let channels = c.channels.list_channels(authed(token, request)).await.unwrap().into_inner().channels;
    (server.id, channels[0].id.clone())
}

async fn send(messages: &mut Messages, token: &str, server_id: &str, channel_id: &str, content: &str) {
    let request = pb::SendMessageRequest {
        server_id: server_id.into(),
        channel_id: channel_id.into(),
        content: content.into(),
        ..Default::default()
    };
    messages.send_message(authed(token, request)).await.unwrap();
}

/// Every message in a channel, paging back from the latest, once its shard
/// answers.
async fn contents(c: &mut Clients, token: &str, server_id: &str, channel_id: &str) -> Vec<String> {
    let mut contents = Vec::new();
    let mut before_id = String::new();
    let mut waited = 0;
    loop {
        let request = pb::ListMessagesRequest {
            server_id: server_id.into(),
            channel_id: channel_id.into(),
            limit: 100,
            before_id: before_id.clone(),
            ..Default::default()
        };
        let page = match c.messages.list_messages(authed(token, request)).await {
            Ok(page) => page.into_inner().messages,
            Err(status) if matches!(status.code(), Code::Unavailable | Code::NotFound) && waited < 100 => {
                waited += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(status) => panic!("{status:?}"),
        };
        let Some(first) = page.first() else { break };
        before_id = first.id.clone();
        contents.extend(page.into_iter().map(|m| m.content));
    }
    contents.sort();
    contents
}

async fn server_names(c: &mut Clients, token: &str) -> Vec<String> {
    let servers = c.servers.list_servers(authed(token, pb::ListServersRequest {})).await.unwrap().into_inner().servers;
    let mut names: Vec<String> = servers.into_iter().map(|s| s.name).collect();
    names.sort();
    names
}

/// The same address on the test's gateway: links are made with the public URL.
fn on(part: &Part, url: &str) -> String {
    format!("http://{}{}", part.addr, reqwest::Url::parse(url).unwrap().path())
}

// Several worker threads, so commits land while the replica ships and folds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_split_instance_comes_back_from_its_replica() {
    let root = tempfile::tempdir().unwrap();
    let replica = root.path().join("replica").to_str().unwrap().to_string();
    let folder = |name: &str| {
        let path = root.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    };

    // The instance as it ran: an account with a direct-message device, two
    // servers (one later deleted), enough messages to fold the shard's log,
    // and a picture.
    let first = folder("first");
    let setup = Setup { root: &first, replica: replica.clone(), restore: false };
    let cluster = setup.start().await;
    let mut c = clients(&cluster.gateway).await;
    let request = pb::SignUpRequest { username: "juan".into(), password: PASSWORD.into(), display_name: String::new() };
    let signed_up = c.auth.sign_up(request).await.unwrap().into_inner();
    let (juan, juan_id) = (signed_up.token, signed_up.user.unwrap().id);
    let device = fuwa_e2ee::Device::new(&juan_id).unwrap();
    let request = pb::RegisterDeviceRequest {
        signature_key: device.signature_key().to_vec(),
        key_packages: device.key_packages(3).unwrap(),
        last_resort_key_package: device.last_resort_key_package().unwrap(),
    };
    c.dms.register_device(authed(&juan, request)).await.unwrap();
    let (kept, general) = create_server(&mut c, &juan, "Kept").await;
    let (gone, _) = create_server(&mut c, &juan, "Gone").await;
    // About 5 MB from four writers at once: more than one log's worth.
    let padding = "x".repeat(3990);
    let mut writers = Vec::new();
    for writer in 0..4 {
        let mut messages = c.messages.clone();
        let (juan, kept, general, padding) = (juan.clone(), kept.clone(), general.clone(), padding.clone());
        writers.push(tokio::spawn(async move {
            let mut sent = Vec::new();
            for i in 0..300 {
                let content = format!("{writer}{i:03} {padding}");
                send(&mut messages, &juan, &kept, &general, &content).await;
                sent.push(content);
            }
            sent
        }));
    }
    let mut sent = Vec::new();
    for writer in writers {
        sent.extend(writer.await.unwrap());
    }
    sent.sort();
    c.servers.delete_server(authed(&juan, pb::DeleteServerRequest { server_id: gone.clone() })).await.unwrap();
    let mut picture = b"\x89PNG\r\n\x1a\n".to_vec();
    picture.resize(300, 7);
    let request = pb::CreateUploadRequest {
        purpose: pb::MediaPurpose::Avatar as i32,
        content_type: "image/png".into(),
        size: picture.len() as i64,
    };
    let reserved = c.media.create_upload(authed(&juan, request)).await.unwrap().into_inner();
    let upload = reqwest::Client::new().put(on(&cluster.gateway, &reserved.upload_url)).body(picture.clone());
    assert_eq!(upload.send().await.unwrap().status(), reqwest::StatusCode::NO_CONTENT);
    let picture_url = reserved.media.unwrap().url;
    cluster.stop().await;
    let folded = std::fs::read_dir(Path::new(&replica).join("servers").join(&kept))
        .unwrap()
        .filter_map(|generation| std::fs::read_dir(generation.unwrap().path()).ok())
        .flatten()
        .map(|object| object.unwrap().file_name().to_string_lossy().into_owned())
        .any(|name| name != "snapshot" && !name.starts_with("00000000-"));
    assert!(folded, "the shard's log was folded into the file at least once");

    // Both volumes are gone. Neither part starts empty over the replica...
    let second = folder("second");
    let setup = Setup { root: &second, replica: replica.clone(), restore: false };
    let (directory, shard) = setup.refusals().await;
    assert!(directory.contains("FUWA_RESTORE=if-empty"), "{directory}");
    assert!(shard.contains("FUWA_RESTORE=if-empty") && shard.contains("shard-1"), "{shard}");
    assert!(!second.join("shard/servers").join(format!("{kept}.db")).exists());

    // ...and with FUWA_RESTORE=if-empty they come back as they were.
    let setup = Setup { restore: true, ..setup };
    let cluster = setup.start().await;
    let mut c = clients(&cluster.gateway).await;
    let juan = sign_in(&mut c).await;
    assert!(contents(&mut c, &juan, &kept, &general).await == sent, "every message came back");
    assert_eq!(server_names(&mut c, &juan).await, ["Kept"], "the deleted server stays deleted");
    let served = reqwest::get(on(&cluster.gateway, &picture_url)).await.unwrap();
    assert_eq!(served.status(), reqwest::StatusCode::OK);
    assert_eq!(served.bytes().await.unwrap().to_vec(), picture);
    let request = pb::ListDevicesRequest { user_ids: vec![juan_id.clone()] };
    let devices = c.dms.list_devices(authed(&juan, request)).await.unwrap().into_inner().devices;
    assert!(
        devices.iter().any(|d| d.signature_key == device.signature_key()),
        "the direct-message device came back with dms.db"
    );

    // They carry on replicating: what's written now survives the next loss.
    send(&mut c.messages, &juan, &kept, &general, "after the restore").await;
    cluster.stop().await;
    let third = folder("third");
    let setup = Setup { root: &third, replica: replica.clone(), restore: true };
    let cluster = setup.start().await;
    let mut c = clients(&cluster.gateway).await;
    let juan = sign_in(&mut c).await;
    let latest = contents(&mut c, &juan, &kept, &general).await;
    assert!(latest.contains(&"after the restore".to_string()));
    assert_eq!(latest.len(), sent.len() + 1);
    cluster.stop().await;

    // A restart on the same volumes restores nothing: the files are there.
    let cluster = setup.start().await;
    assert_eq!(cluster.shard.app.as_ref().unwrap().servers.len(), 1);
    cluster.stop().await;
}
