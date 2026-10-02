//! A gateway: what clients reach in a split instance. It keeps no data. It
//! serves the web app, answers browsers (CORS, gRPC-Web), and passes each
//! call on: to the directory, or to the shard holding the call's server (the
//! `server_id` every server-scoped request carries as its field 1). A live
//! event stream following servers on several shards is one stream per shard,
//! merged here.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use futures::{Stream, StreamExt};
use http::{HeaderName, StatusCode, header};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::codegen::Service;
use tonic::metadata::{AsciiMetadataValue, MetadataMap};
use tonic::transport::Channel;
use tonic::{Status, Streaming};

use super::{Backoff, DirectoryClient, KEY_HEADER, Keyed, WithKey, forward_metadata};
use crate::app::{HasSettings, cors};
use crate::config::Config;
use crate::cpb;
use crate::error::{Error, MISROUTED};
use crate::id::{new_id, now_ms, parse_id, timestamp};
use crate::pb::{self, event_service_server::EventService};
use crate::servers::Payload;
use crate::settings::Settings;

/// The largest request passed on. Client messages are far smaller; this is
/// only a backstop.
const MAX_REQUEST: usize = 64 * 1024 * 1024;
/// Most servers one stream can follow, as a single process allows.
const MAX_SERVERS: usize = 200;
/// Merged streams pass on at most one heartbeat this often, however many
/// shards send them.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(20);

/// Where a call goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Directory,
    /// The shard holding the server named in the request's field 1.
    Shard,
    Unknown,
}

/// Which part answers each call, by its gRPC path. The event service is
/// answered here (see [`Events`]).
fn route(path: &str) -> Target {
    let Some((service, method)) = path.trim_start_matches('/').split_once('/') else { return Target::Unknown };
    match service {
        "fuwa.v1.NodeService" | "fuwa.v1.AuthService" | "fuwa.v1.AccountService" | "fuwa.v1.MediaService" => {
            Target::Directory
        }
        "fuwa.v1.AdminService" if matches!(method, "SetServerLimits" | "ExportServer") => Target::Shard,
        "fuwa.v1.AdminService" => Target::Directory,
        "fuwa.v1.ServerService" if matches!(method, "CreateServer" | "ListServers" | "DiscoverServers") => {
            Target::Directory
        }
        // Only the directory knows which server a code is for.
        "fuwa.v1.InviteService" if method == "GetInvite" => Target::Directory,
        "fuwa.v1.ServerService"
        | "fuwa.v1.ChannelService"
        | "fuwa.v1.MessageService"
        | "fuwa.v1.RoleService"
        | "fuwa.v1.InviteService"
        | "fuwa.v1.JoinService" => Target::Shard,
        _ => Target::Unknown,
    }
}

/// The `server_id` in a request: field 1 of the first message, as a string.
fn server_id_of(body: &[u8]) -> Option<String> {
    let (&compressed, rest) = body.split_first()?;
    if compressed != 0 || rest.len() < 4 {
        return None;
    }
    let length = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
    let mut message = rest.get(4..4 + length)?;
    while !message.is_empty() {
        let key = varint(&mut message)?;
        let (field, wire) = (key >> 3, key & 7);
        let skip = match wire {
            0 => {
                varint(&mut message)?;
                0
            }
            1 => 8,
            2 => {
                let length = varint(&mut message)? as usize;
                if field == 1 {
                    return String::from_utf8(message.get(..length)?.to_vec()).ok();
                }
                length
            }
            5 => 4,
            _ => return None,
        };
        message = message.get(skip..)?;
    }
    // Field 1 left unset: an empty id, which the shard would refuse anyway.
    Some(String::new())
}

fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Where a server is.
#[derive(Debug, Clone)]
struct Placement {
    url: String,
}

pub struct Gateway {
    config: Config,
    key: AsciiMetadataValue,
    directory_url: String,
    /// Raw calls to the directory, passed on as they came.
    directory_channel: Channel,
    directory: DirectoryClient,
    settings: watch::Sender<Arc<Settings>>,
    placements: RwLock<HashMap<String, Placement>>,
    /// Connections to shards, by URL.
    shards: RwLock<HashMap<String, Channel>>,
    shutdown: CancellationToken,
}

impl HasSettings for Gateway {
    fn settings(&self) -> Arc<Settings> {
        self.settings.borrow().clone()
    }
}

/// Serves a gateway until Ctrl-C or SIGTERM.
pub async fn run(config: Config, address: SocketAddr) -> Result<(), String> {
    let listener =
        tokio::net::TcpListener::bind(address).await.map_err(|err| format!("couldn't listen on {address}: {err}"))?;
    let gateway = Gateway::new(config).map_err(|err| err.to_string())?;
    tracing::info!(version = crate::VERSION, role = "gateway", %address, directory = %gateway.directory_url, "fuwa is up");
    crate::app::spawn_signal_handler(gateway.shutdown.clone());
    let shutdown = gateway.shutdown.clone();
    axum::serve(listener, gateway.router())
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .map_err(|err| format!("server error: {err}"))
}

impl Gateway {
    /// A gateway following the directory's settings (the environment's until
    /// it has them).
    pub fn new(config: Config) -> crate::error::Result<Arc<Self>> {
        let cluster = &config.cluster;
        let directory_url = cluster.directory_url.clone().ok_or_else(|| Error::internal("no directory URL"))?;
        let key = cluster.key_value()?;
        let directory_channel = super::channel(&directory_url)?;
        let directory = cpb::directory_service_client::DirectoryServiceClient::with_interceptor(
            directory_channel.clone(),
            WithKey(key.clone()),
        );
        let gateway = Arc::new(Self {
            settings: watch::Sender::new(Arc::new(Settings::defaults(&config))),
            config,
            key,
            directory_url,
            directory_channel,
            directory,
            placements: RwLock::new(HashMap::new()),
            shards: RwLock::new(HashMap::new()),
            shutdown: CancellationToken::new(),
        });
        tokio::spawn(follow_settings(gateway.clone()));
        Ok(gateway)
    }

    pub fn shutdown(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Calls pass through gRPC-Web (for browsers) and on to the other parts;
    /// everything else is the web app, pictures and health.
    pub fn router(self: &Arc<Self>) -> Router {
        let (_, health) = tonic_health::server::health_reporter();
        let reflection = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(crate::proto::FILE_DESCRIPTOR_SET)
            .build_v1()
            .expect("the embedded descriptor set is valid");
        let forward = self.clone();
        let grpc = tonic::service::Routes::new(pb::event_service_server::EventServiceServer::new(Events(self.clone())))
            .add_service(health)
            .add_service(reflection)
            .into_axum_router()
            .fallback(move |request: Request| {
                let gateway = forward.clone();
                async move { gateway.forward(request).await }
            })
            .layer(tonic_web::GrpcWebLayer::new());

        let media = self.clone();
        let http = Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route(
                "/media/{*rest}",
                any(move |request: Request| {
                    let gateway = media.clone();
                    async move { gateway.pass(gateway.directory_channel.clone(), request).await }
                }),
            )
            .fallback(crate::web::handler(self.clone()));

        Router::new()
            .fallback(move |request: Request| {
                let (mut grpc, mut http) = (grpc.clone(), http.clone());
                async move {
                    let is_grpc = request
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| value.starts_with("application/grpc"));
                    let response = if is_grpc { grpc.call(request).await } else { http.call(request).await };
                    response.unwrap_or_else(|never| match never {})
                }
            })
            .layer(cors(self.clone()))
    }

    /// Passes a call on to whichever part answers it.
    async fn forward(&self, request: Request) -> Response {
        let target = route(request.uri().path());
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, MAX_REQUEST).await {
            Ok(body) => body,
            Err(_) => return grpc_error(Status::resource_exhausted("that request is too big")),
        };
        match target {
            Target::Unknown => grpc_error(Status::unimplemented(format!("{} isn't a fuwa call", parts.uri.path()))),
            Target::Directory => match self.send(self.directory_channel.clone(), &parts, body).await {
                Ok(response) => response,
                Err(err) => grpc_error(unreachable_part(&err)),
            },
            Target::Shard => {
                let Some(server_id) = server_id_of(&body) else {
                    return grpc_error(Status::invalid_argument("couldn't read that request"));
                };
                if let Err(err) = parse_id("server_id", &server_id) {
                    return grpc_error(err.into());
                }
                for attempt in 0..2 {
                    let refresh = attempt > 0;
                    let channel = match self.shard_for(&server_id, refresh).await {
                        Ok(channel) => channel,
                        Err(status) if !refresh && status.code() == tonic::Code::Unavailable => continue,
                        Err(status) => return grpc_error(status),
                    };
                    match self.send(channel, &parts, body.clone()).await {
                        Ok(response) if !refresh && response.headers().contains_key(MISROUTED) => continue,
                        Ok(response) => return response,
                        Err(_) if !refresh => continue,
                        Err(err) => return grpc_error(unreachable_part(&err)),
                    }
                }
                grpc_error(Status::unavailable("that server is moving between shards; try again soon"))
            }
        }
    }

    /// Sends a request on as it came, with the cluster key.
    async fn send(
        &self,
        mut channel: Channel,
        parts: &http::request::Parts,
        body: Bytes,
    ) -> Result<Response, tonic::transport::Error> {
        let mut request = http::Request::new(tonic::body::Body::new(Body::from(body)));
        *request.method_mut() = parts.method.clone();
        *request.uri_mut() = parts.uri.clone();
        *request.headers_mut() = passed_headers(&parts.headers, &self.key);
        std::future::poll_fn(|cx| channel.poll_ready(cx)).await?;
        Ok(channel.call(request).await?.map(Body::new))
    }

    /// Passes a plain HTTP request (a picture, an upload) on, streaming.
    async fn pass(&self, mut channel: Channel, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let mut request = http::Request::new(tonic::body::Body::new(body));
        *request.method_mut() = parts.method;
        *request.uri_mut() = parts.uri;
        *request.headers_mut() = passed_headers(&parts.headers, &self.key);
        let sent = async {
            std::future::poll_fn(|cx| channel.poll_ready(cx)).await?;
            channel.call(request).await
        };
        match sent.await {
            Ok(response) => response.map(Body::new),
            Err(err) => {
                tracing::warn!(error = %err, "couldn't reach the directory");
                (StatusCode::BAD_GATEWAY, "part of this instance is unreachable right now; try again soon\n")
                    .into_response()
            }
        }
    }

    fn shard_channel(&self, url: &str) -> Result<Channel, Status> {
        if let Some(channel) = self.shards.read().unwrap_or_else(|p| p.into_inner()).get(url) {
            return Ok(channel.clone());
        }
        let channel = super::channel(url).map_err(Status::from)?;
        self.shards.write().unwrap_or_else(|p| p.into_inner()).insert(url.to_string(), channel.clone());
        Ok(channel)
    }

    fn shard_client(&self, url: &str) -> Result<pb::event_service_client::EventServiceClient<Keyed>, Status> {
        let channel =
            tonic::service::interceptor::InterceptedService::new(self.shard_channel(url)?, WithKey(self.key.clone()));
        Ok(pb::event_service_client::EventServiceClient::new(channel).max_decoding_message_size(MAX_REQUEST))
    }

    /// Where these servers are: remembered, or asked of the directory (always,
    /// with `refresh`). Servers that don't exist are left out.
    async fn placements(&self, ids: &[String], refresh: bool) -> Result<HashMap<String, Placement>, Status> {
        let mut found = HashMap::new();
        let mut missing = Vec::new();
        {
            let known = self.placements.read().unwrap_or_else(|p| p.into_inner());
            for id in ids {
                match known.get(id).filter(|_| !refresh) {
                    Some(placement) => {
                        found.insert(id.clone(), placement.clone());
                    }
                    None => missing.push(id.clone()),
                }
            }
        }
        if missing.is_empty() {
            return Ok(found);
        }
        let answer = self.directory.clone().placements(cpb::PlacementsRequest { server_ids: missing.clone() }).await;
        let answer = answer.map_err(|status| Status::from(Error::from(status)))?.into_inner();
        let mut known = self.placements.write().unwrap_or_else(|p| p.into_inner());
        for id in &missing {
            known.remove(id);
        }
        for placement in answer.placements {
            let entry = Placement { url: placement.url };
            // A shard that's down isn't remembered, so the next call asks again.
            if !entry.url.is_empty() {
                known.insert(placement.server_id.clone(), entry.clone());
            }
            found.insert(placement.server_id, entry);
        }
        Ok(found)
    }

    /// The connection to the shard holding a server.
    async fn shard_for(&self, server_id: &str, refresh: bool) -> Result<Channel, Status> {
        let placements = self.placements(&[server_id.to_string()], refresh).await?;
        let placement = placements.get(server_id).ok_or_else(|| Status::not_found("server not found"))?;
        if placement.url.is_empty() {
            return Err(Status::unavailable("the part of this instance holding that server is down; try again soon"));
        }
        self.shard_channel(&placement.url)
    }

    fn forget(&self, server_ids: &[String]) {
        let mut known = self.placements.write().unwrap_or_else(|p| p.into_inner());
        for id in server_ids {
            known.remove(id);
        }
    }

    /// Which account is calling, as the directory says. Only signed-in
    /// accounts follow servers.
    async fn caller(&self, metadata: &MetadataMap) -> Result<String, Status> {
        let token = crate::auth::bearer(metadata).ok_or_else(|| Status::from(Error::Unauthenticated))?;
        let found = self.directory.clone().authenticate(cpb::AuthenticateRequest { token: token.to_string() }).await;
        let found = found.map_err(|status| Status::from(Error::from(status)))?.into_inner();
        match found.account {
            Some(account) => Ok(account.id),
            None => Err(Error::denied("the admin token can't act as an account; sign in instead").into()),
        }
    }
}

/// Follows the directory's settings for as long as the gateway runs.
async fn follow_settings(gateway: Arc<Gateway>) {
    let mut backoff = Backoff::default();
    let mut connected = false;
    loop {
        let mut directory = gateway.directory.clone();
        let watch = directory.watch(cpb::WatchRequest { shard_id: String::new() });
        let watching = tokio::select! {
            _ = gateway.shutdown.cancelled() => return,
            watching = watch => watching,
        };
        match watching {
            Ok(response) => {
                let mut stream = response.into_inner();
                loop {
                    let message = tokio::select! {
                        _ = gateway.shutdown.cancelled() => return,
                        message = stream.message() => message,
                    };
                    match message {
                        Ok(Some(message)) => {
                            if !connected {
                                connected = true;
                                tracing::info!(directory = %gateway.directory_url, "following the directory");
                            }
                            backoff.reset();
                            if let Some(settings) = message.settings {
                                gateway.settings.send_replace(Arc::new(Settings::from_pb(&gateway.config, &settings)));
                            }
                        }
                        Ok(None) | Err(_) => {
                            connected = false;
                            tracing::warn!("lost the directory");
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(directory = %gateway.directory_url, error = %err.message(), "can't reach the directory")
            }
        }
        tokio::select! {
            _ = gateway.shutdown.cancelled() => return,
            _ = tokio::time::sleep(backoff.wait()) => {}
        }
    }
}

/// The headers a call is passed on with: the caller's, less those about the
/// connection it came in on, plus the cluster key.
fn passed_headers(from: &http::HeaderMap, key: &AsciiMetadataValue) -> http::HeaderMap {
    let mut headers = http::HeaderMap::with_capacity(from.len() + 1);
    for (name, value) in from {
        let hop = matches!(name.as_str(), "host" | "connection" | "keep-alive" | "transfer-encoding" | "upgrade");
        if !hop && name.as_str() != KEY_HEADER {
            headers.append(name.clone(), value.clone());
        }
    }
    let value = http::HeaderValue::from_bytes(key.as_encoded_bytes()).expect("the cluster key is a valid header");
    headers.insert(HeaderName::from_static(KEY_HEADER), value);
    headers
}

fn grpc_error(status: Status) -> Response {
    status.into_http::<Body>()
}

fn unreachable_part(err: &tonic::transport::Error) -> Status {
    tracing::warn!(error = %err, "a part of this instance didn't answer");
    Status::unavailable("part of this instance is unreachable right now; try again soon")
}

/// The event service, answered by the gateway: live streams merge the
/// streams of every shard involved.
#[derive(Clone)]
struct Events(Arc<Gateway>);

type EventStream = Pin<Box<dyn Stream<Item = Result<pb::SubscribeResponse, Status>> + Send>>;

/// One shard's stream, numbered, ending with `None`.
type Tagged = Pin<Box<dyn Stream<Item = (usize, Option<Result<pb::SubscribeResponse, Status>>)> + Send>>;

fn gone_event(server_id: String) -> pb::SubscribeResponse {
    pb::SubscribeResponse {
        event: Some(pb::Event {
            id: new_id(),
            server_id,
            sequence: 0,
            actor_id: String::new(),
            created_at: Some(timestamp(now_ms())),
            payload: Some(Payload::ServerDeleted(pb::ServerDeleted {})),
        }),
        ready: None,
    }
}

#[tonic::async_trait]
impl EventService for Events {
    type SubscribeStream = EventStream;

    async fn subscribe(
        &self,
        request: tonic::Request<pb::SubscribeRequest>,
    ) -> Result<tonic::Response<EventStream>, Status> {
        let gateway = &self.0;
        let metadata = request.metadata().clone();
        let cursors = request.into_inner().servers;
        if cursors.is_empty() || cursors.len() > MAX_SERVERS {
            return Err(Error::invalid(format!("follow 1 to {MAX_SERVERS} servers per stream")).into());
        }
        let account_id = gateway.caller(&metadata).await?;
        let mut ids = Vec::with_capacity(cursors.len());
        for cursor in &cursors {
            ids.push(parse_id("server_id", &cursor.server_id).map_err(Status::from)?);
        }

        // Each shard's share of the servers; servers that no longer exist get
        // the event that says so, so the client lets them go.
        let mut opened: Vec<Tagged> = Vec::new();
        // The servers each opened stream still follows.
        let mut following: Vec<HashSet<String>> = Vec::new();
        let mut gone = Vec::new();
        for attempt in 0..2 {
            let refresh = attempt > 0;
            let placements = gateway.placements(&ids, refresh).await?;
            let mut by_shard: HashMap<String, Vec<pb::ServerCursor>> = HashMap::new();
            gone.clear();
            for cursor in &cursors {
                match placements.get(&cursor.server_id) {
                    None => gone.push(gone_event(cursor.server_id.clone())),
                    Some(placement) if placement.url.is_empty() => {
                        return Err(Status::unavailable(
                            "the part of this instance holding one of these servers is down; try again soon",
                        ));
                    }
                    Some(placement) => by_shard.entry(placement.url.clone()).or_default().push(cursor.clone()),
                }
            }
            opened.clear();
            following.clear();
            let mut misrouted = false;
            for (index, (url, servers)) in by_shard.into_iter().enumerate() {
                let ids = servers.iter().map(|cursor| cursor.server_id.clone()).collect();
                let request = forward_metadata(&metadata, pb::SubscribeRequest { servers });
                match gateway.shard_client(&url)?.subscribe(request).await {
                    Ok(stream) => {
                        opened.push(tag(index, stream.into_inner()));
                        following.push(ids);
                    }
                    Err(status) if status.metadata().get(MISROUTED).is_some() || status.source_is_transport() => {
                        misrouted = true;
                        break;
                    }
                    Err(status) => return Err(status),
                }
            }
            if !misrouted {
                break;
            }
            gateway.forget(&ids);
            if refresh {
                return Err(Status::unavailable("these servers are moving between shards; try again soon"));
            }
        }

        let (tx, rx) = mpsc::channel::<Result<pb::SubscribeResponse, Status>>(256);
        let shutdown = gateway.shutdown();
        tokio::spawn(async move {
            for event in gone {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            if opened.is_empty() {
                let _ = tx
                    .send(Ok(pb::SubscribeResponse { event: None, ready: Some(pb::SubscribeReady::default()) }))
                    .await;
                return;
            }
            let mut waiting = opened.len();
            let mut heads = Vec::new();
            let mut merged = futures::stream::select_all(opened);
            let mut last_heartbeat = Instant::now();
            loop {
                let item = tokio::select! {
                    _ = shutdown.cancelled() => {
                        // Stopping, say for a deploy: tell the client to follow again
                        // rather than end the stream as if it were done.
                        let _ = tx.try_send(Err(Status::unavailable(
                            "this instance is restarting; subscribe again from your last sequence",
                        )));
                        return;
                    }
                    _ = tx.closed() => return,
                    item = merged.next() => item,
                };
                let message = match item {
                    None => return,
                    // A shard's stream ends by itself once every server it
                    // followed is gone. Ending sooner, it went away (say, to
                    // restart), so the client follows again from where it got to.
                    Some((index, None)) if following[index].is_empty() => continue,
                    Some((_, None)) => {
                        Err(Status::unavailable("lost touch with part of this instance; subscribe again"))
                    }
                    Some((_, Some(Err(status)))) => Err(status),
                    Some((index, Some(Ok(response)))) => match (response.event, response.ready) {
                        (_, Some(ready)) => {
                            heads.extend(ready.servers);
                            waiting = waiting.saturating_sub(1);
                            if waiting > 0 {
                                continue;
                            }
                            Ok(pb::SubscribeResponse {
                                event: None,
                                ready: Some(pb::SubscribeReady { servers: std::mem::take(&mut heads) }),
                            })
                        }
                        (Some(event), None) => {
                            let ends = match &event.payload {
                                Some(Payload::ServerDeleted(_)) => true,
                                Some(Payload::MemberLeft(left)) => left.user_id == account_id,
                                _ => false,
                            };
                            if ends {
                                following[index].remove(&event.server_id);
                            }
                            Ok(pb::SubscribeResponse { event: Some(event), ready: None })
                        }
                        (None, None) => {
                            if waiting > 0 || last_heartbeat.elapsed() < HEARTBEAT_EVERY {
                                continue;
                            }
                            last_heartbeat = Instant::now();
                            Ok(pb::SubscribeResponse::default())
                        }
                    },
                };
                let failed = message.is_err();
                if tx.send(message).await.is_err() || failed {
                    return;
                }
            }
        });
        Ok(tonic::Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn list_events(
        &self,
        request: tonic::Request<pb::ListEventsRequest>,
    ) -> Result<tonic::Response<pb::ListEventsResponse>, Status> {
        let gateway = &self.0;
        let server_id = parse_id("server_id", &request.get_ref().server_id).map_err(Status::from)?;
        let metadata = request.metadata().clone();
        let message = request.into_inner();
        for attempt in 0..2 {
            let refresh = attempt > 0;
            let placements = gateway.placements(std::slice::from_ref(&server_id), refresh).await?;
            let placement = placements.get(&server_id).ok_or_else(|| Status::not_found("server not found"))?;
            if placement.url.is_empty() {
                return Err(Status::unavailable(
                    "the part of this instance holding that server is down; try again soon",
                ));
            }
            let request = forward_metadata(&metadata, message.clone());
            match gateway.shard_client(&placement.url)?.list_events(request).await {
                Err(status)
                    if !refresh && (status.metadata().get(MISROUTED).is_some() || status.source_is_transport()) =>
                {
                    gateway.forget(std::slice::from_ref(&server_id));
                }
                answer => return answer,
            }
        }
        Err(Status::unavailable("that server is moving between shards; try again soon"))
    }
}

fn tag(index: usize, stream: Streaming<pb::SubscribeResponse>) -> Tagged {
    Box::pin(stream.map(move |item| (index, Some(item))).chain(futures::stream::once(async move { (index, None) })))
}

/// Whether a failed call never reached the other side.
trait TransportFailure {
    fn source_is_transport(&self) -> bool;
}

impl TransportFailure for Status {
    fn source_is_transport(&self) -> bool {
        std::error::Error::source(self).is_some()
    }
}

#[cfg(test)]
mod tests {
    use prost::Message as _;

    use super::*;

    fn framed(message: impl prost::Message) -> Vec<u8> {
        let bytes = message.encode_to_vec();
        let mut framed = vec![0];
        framed.extend((bytes.len() as u32).to_be_bytes());
        framed.extend(bytes);
        framed
    }

    #[test]
    fn reads_the_server_id() {
        let request =
            pb::SendMessageRequest { server_id: "01J0SERVER".into(), content: "hi".into(), ..Default::default() };
        assert_eq!(server_id_of(&framed(request)).as_deref(), Some("01J0SERVER"));
        let reorder = pb::ReorderChannelsRequest { server_id: "s".into(), ..Default::default() };
        assert_eq!(server_id_of(&framed(reorder)).as_deref(), Some("s"));
        assert_eq!(server_id_of(&framed(pb::GetServerRequest::default())).as_deref(), Some(""));
        assert_eq!(server_id_of(&[1, 0, 0, 0, 0]), None);
        assert_eq!(server_id_of(&[0, 0, 0, 0, 9, 1]), None);
    }

    /// Every call has somewhere to go, and each one sent to a shard names its
    /// server in field 1, which is how the gateway finds the shard.
    #[test]
    fn every_call_is_routed() {
        let set = prost_types::FileDescriptorSet::decode(crate::proto::FILE_DESCRIPTOR_SET).unwrap();
        let messages: HashMap<String, &prost_types::DescriptorProto> = set
            .file
            .iter()
            .flat_map(|file| file.message_type.iter().map(move |m| (format!(".{}.{}", file.package(), m.name()), m)))
            .collect();
        let mut routed = 0;
        for file in &set.file {
            for service in &file.service {
                let name = format!("{}.{}", file.package(), service.name());
                if name == "fuwa.v1.EventService" {
                    continue;
                }
                for method in &service.method {
                    let path = format!("/{name}/{}", method.name());
                    let target = route(&path);
                    assert_ne!(target, Target::Unknown, "{path} isn't routed");
                    if target == Target::Shard {
                        let input = messages[method.input_type()];
                        let first = input.field.iter().find(|f| f.number() == 1);
                        assert!(
                            first.is_some_and(|f| f.name() == "server_id"
                                && f.r#type() == prost_types::field_descriptor_proto::Type::String),
                            "{path} goes to a shard but its field 1 isn't server_id"
                        );
                    }
                    routed += 1;
                }
            }
        }
        assert!(routed > 50);
    }
}
