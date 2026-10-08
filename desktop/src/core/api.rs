//! Talking to one instance: the generated gRPC clients, over gRPC-Web.
//!
//! gRPC-Web is what the web app speaks too, and it goes through any proxy an
//! instance sits behind (HTTP/1.1, no trailers), which plain gRPC doesn't
//! always. Every call carries the session's bearer token, when there is one.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use http::{HeaderValue, Request, Uri};
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use parking_lot::RwLock;
use tonic::body::Body;
use tonic::{Code, Status};
use tonic_web::{GrpcWebCall, GrpcWebClientLayer, GrpcWebClientService};
use tower::{Layer, Service};

use crate::core::reports;
use crate::pb;

/// How long a call may take before it counts as lost.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);

type Https = Framed<Client<HttpsConnector<HttpConnector>, GrpcWebCall<Body>>>;

/// The transport every client shares: gRPC-Web over HTTP/1.1, with or
/// without TLS, adding the session's token to each call.
#[derive(Clone)]
pub struct Transport {
    inner: GrpcWebClientService<Https>,
    token: Arc<RwLock<Option<String>>>,
}

impl Service<Request<Body>> for Transport {
    type Response = <GrpcWebClientService<Https> as Service<Request<Body>>>::Response;
    type Error = <GrpcWebClientService<Https> as Service<Request<Body>>>::Error;
    type Future = <GrpcWebClientService<Https> as Service<Request<Body>>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        if let Some(token) = self.token.read().as_deref()
            && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            req.headers_mut().insert(http::header::AUTHORIZATION, value);
        }
        req.headers_mut().insert(http::header::USER_AGENT, USER_AGENT.clone());
        self.inner.call(req)
    }
}

/// Hands tonic-web the response one whole gRPC-Web frame at a time.
///
/// tonic-web (0.14.6) loses the status when a message and the trailers frame
/// after it arrive in one chunk: it keeps the trailers aside, gives the
/// message, then ends the body without them ("missing grpc-status trailer").
/// Proxies that buffer (Cloudflare in front of fuwa.chat) send just that, so
/// every call through them failed. Split at frame edges, the trailers always
/// come alone and are read as they should be.
#[derive(Clone)]
pub struct Framed<S>(S);

impl<S, B, R> Service<Request<B>> for Framed<S>
where
    S: Service<Request<B>, Response = http::Response<R>>,
    S::Future: Send + 'static,
{
    type Response = http::Response<Frames<R>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let future = self.0.call(req);
        Box::pin(async move { Ok(future.await?.map(|body| Frames { body, pending: BytesMut::new(), done: false })) })
    }
}

/// A response body cut into whole gRPC-Web frames (see [`Framed`]).
pub struct Frames<B> {
    body: B,
    pending: BytesMut,
    done: bool,
}

impl<B> Frames<B> {
    /// The first whole frame waiting, if there is one.
    fn next_frame(&mut self) -> Option<Bytes> {
        let header: [u8; 5] = self.pending.get(..5)?.try_into().ok()?;
        let len = 5 + u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        (self.pending.len() >= len).then(|| self.pending.split_to(len).freeze())
    }
}

impl<B> HttpBody for Frames<B>
where
    B: HttpBody<Data = Bytes> + Unpin,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = self.get_mut();
        loop {
            if let Some(frame) = this.next_frame() {
                return Poll::Ready(Some(Ok(Frame::data(frame))));
            }
            if this.done {
                // Whatever's left isn't a whole frame: tonic-web says what's wrong with it.
                let rest = this.pending.split().freeze();
                return Poll::Ready((!rest.is_empty()).then(|| Ok(Frame::data(rest))));
            }
            match ready!(Pin::new(&mut this.body).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => this.pending.extend_from_slice(&data),
                    // HTTP trailers (gRPC-Web never sends them, but pass them on).
                    Err(frame) => return Poll::Ready(Some(Ok(frame))),
                },
                Some(Err(error)) => return Poll::Ready(Some(Err(error))),
                None => this.done = true,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done && self.pending.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

/// What the instance shows in your list of signed-in devices.
static USER_AGENT: std::sync::LazyLock<HeaderValue> = std::sync::LazyLock::new(|| {
    let agent =
        format!("fuwa-desktop/{} ({}; {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    HeaderValue::from_str(&agent).unwrap_or(HeaderValue::from_static("fuwa-desktop"))
});

/// The client for one instance.
#[derive(Clone)]
pub struct Api {
    pub url: String,
    origin: Uri,
    transport: Transport,
    token: Arc<RwLock<Option<String>>>,
}

macro_rules! clients {
    ($($name:ident => $service:literal $client:ty),* $(,)?) => {
        impl Api {
            $(pub fn $name(&self) -> $client {
                <$client>::with_origin(self.transport.clone(), self.origin.clone())
                    .max_decoding_message_size(32 * 1024 * 1024)
            })*
        }
        $(impl GrpcService for $client {
            const NAME: &'static str = $service;
        })*
    };
}

/// A client's gRPC service, as the protocol names it ("MessageService").
pub trait GrpcService {
    const NAME: &'static str;
}

/// The service a client calls, for naming its calls in reports.
pub fn service_of<C: GrpcService>(_: &C) -> &'static str {
    C::NAME
}

clients! {
    node => "NodeService" pb::node_service_client::NodeServiceClient<Transport>,
    auth => "AuthService" pb::auth_service_client::AuthServiceClient<Transport>,
    account => "AccountService" pb::account_service_client::AccountServiceClient<Transport>,
    servers => "ServerService" pb::server_service_client::ServerServiceClient<Transport>,
    channels => "ChannelService" pb::channel_service_client::ChannelServiceClient<Transport>,
    shared => "SharedChannelService" pb::shared_channel_service_client::SharedChannelServiceClient<Transport>,
    messages => "MessageService" pb::message_service_client::MessageServiceClient<Transport>,
    events => "EventService" pb::event_service_client::EventServiceClient<Transport>,
    roles => "RoleService" pb::role_service_client::RoleServiceClient<Transport>,
    emojis => "EmojiService" pb::emoji_service_client::EmojiServiceClient<Transport>,
    webhooks => "WebhookService" pb::webhook_service_client::WebhookServiceClient<Transport>,
    agents => "AgentService" pb::agent_service_client::AgentServiceClient<Transport>,
    automod => "AutoModService" pb::auto_mod_service_client::AutoModServiceClient<Transport>,
    invites => "InviteService" pb::invite_service_client::InviteServiceClient<Transport>,
    dms => "DirectMessageService" pb::direct_message_service_client::DirectMessageServiceClient<Transport>,
    secure => "SecureChannelService" pb::secure_channel_service_client::SecureChannelServiceClient<Transport>,
    media => "MediaService" pb::media_service_client::MediaServiceClient<Transport>,
    friends => "FriendService" pb::friend_service_client::FriendServiceClient<Transport>,
    search => "SearchService" pb::search_service_client::SearchServiceClient<Transport>,
    join => "JoinService" pb::join_service_client::JoinServiceClient<Transport>,
    calls => "CallService" pb::call_service_client::CallServiceClient<Transport>,
    sso => "SsoService" pb::sso_service_client::SsoServiceClient<Transport>,
    presence => "PresenceService" pb::presence_service_client::PresenceServiceClient<Transport>,
    admin => "AdminService" pb::admin_service_client::AdminServiceClient<Transport>,
    commands => "CommandService" pb::command_service_client::CommandServiceClient<Transport>,
    gifs => "GifService" pb::gif_service_client::GifServiceClient<Transport>,
    live_tiles => "LiveTileService" pb::live_tile_service_client::LiveTileServiceClient<Transport>,
    profile_items => "ProfileItemService" pb::profile_item_service_client::ProfileItemServiceClient<Transport>,
}

impl Api {
    /// A client for the instance at `url` (already normalized, see [`normalize_url`]).
    pub fn new(url: &str, token: Option<String>) -> anyhow::Result<Self> {
        let origin: Uri = url.parse()?;
        // The system's certificates (so a company's own authority works too), or
        // the public ones when the system has none.
        let roots = match hyper_rustls::HttpsConnectorBuilder::new().with_native_roots() {
            Ok(roots) => roots,
            Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
        };
        let connector = roots.https_or_http().enable_http1().build();
        let client = Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(60)).build(connector);
        let token = Arc::new(RwLock::new(token));
        let transport = Transport { inner: GrpcWebClientLayer::new().layer(Framed(client)), token: token.clone() };
        Ok(Self { url: url.to_owned(), origin, transport, token })
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().clone()
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write() = token;
    }
}

/// What went wrong with a call, in words a person can read.
#[derive(Debug, Clone)]
pub struct Problem {
    pub code: Code,
    pub message: String,
}

impl Problem {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// The session ended: signed out elsewhere, revoked, or the account turned off.
    pub fn signed_out(&self) -> bool {
        self.code == Code::Unauthenticated
    }

    /// Worth trying again: the instance (or the way to it) is down for now.
    pub fn retryable(&self) -> bool {
        matches!(self.code, Code::Unavailable | Code::DeadlineExceeded | Code::Unknown | Code::Internal | Code::Aborted)
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Problem {}

impl From<Status> for Problem {
    fn from(status: Status) -> Self {
        let message = match status.code() {
            Code::Unavailable | Code::Unknown if unreachable(&status) => {
                "Couldn't reach this instance. Check the address and your connection.".to_owned()
            }
            Code::Unavailable if status.message().is_empty() => "The instance isn't answering right now.".to_owned(),
            Code::DeadlineExceeded => "The instance took too long to answer.".to_owned(),
            Code::Unauthenticated => "Your session ended. Sign in again.".to_owned(),
            Code::Unimplemented => "This instance doesn't support that yet.".to_owned(),
            _ if status.message().is_empty() => format!("Something went wrong ({:?}).", status.code()),
            _ => capitalize(status.message()),
        };
        Self { code: status.code(), message }
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Makes a call on a fresh client (`rpc!(api.node(), get_node(request))`),
/// with the timeout every call gets, timed for the anonymous reports; see
/// [`call_named`].
#[macro_export]
macro_rules! rpc {
    ($client:expr, $method:ident($request:expr) $(,)?) => {{
        // Named once per place it's called from: "rpc:MessageService/SendMessage".
        static METRIC: std::sync::OnceLock<Box<str>> = std::sync::OnceLock::new();
        let mut client = $client;
        let request = $request;
        let metric = METRIC.get_or_init(|| {
            $crate::core::reports::rpc_metric($crate::core::api::service_of(&client), stringify!($method))
        });
        $crate::core::api::call_named(metric, async move { client.$method(request).await })
    }};
}

/// Runs a call with the timeout every call gets, giving its response or a [`Problem`].
pub async fn call<T>(future: impl Future<Output = Result<tonic::Response<T>, Status>>) -> Result<T, Problem> {
    match tokio::time::timeout(CALL_TIMEOUT, future).await {
        Ok(Ok(response)) => Ok(response.into_inner()),
        Ok(Err(status)) => Err(status.into()),
        Err(_) => Err(Problem::new(Code::DeadlineExceeded, "The instance took too long to answer.")),
    }
}

/// [`call`], timed as `metric` ("rpc:Service/Method"), counting the failures
/// that happened inside the instance (not the ones on the way to it) as
/// `rpc_internal` at that method.
pub async fn call_named<T>(
    metric: &'static str,
    future: impl Future<Output = Result<tonic::Response<T>, Status>>,
) -> Result<T, Problem> {
    if !reports::enabled() {
        return call(future).await;
    }
    let started = std::time::Instant::now();
    let result = match tokio::time::timeout(CALL_TIMEOUT, future).await {
        Ok(Ok(response)) => Ok(response.into_inner()),
        Ok(Err(status)) => {
            if broke_inside(&status) {
                reports::error("rpc_internal", metric.strip_prefix("rpc:").unwrap_or(metric));
            }
            Err(status.into())
        }
        Err(_) => Err(Problem::new(Code::DeadlineExceeded, "The instance took too long to answer.")),
    };
    reports::timing(metric, started.elapsed());
    result
}

/// A failure inside the instance: Internal, DataLoss, or Unknown when it
/// isn't the connection failing.
fn broke_inside(status: &Status) -> bool {
    match status.code() {
        Code::Internal | Code::DataLoss => true,
        Code::Unknown => !unreachable(status),
        _ => false,
    }
}

/// The instance couldn't be reached at all.
fn unreachable(status: &Status) -> bool {
    let message = status.message();
    message.contains("error trying to connect")
        || message.contains("tcp connect")
        || message.contains("dns error")
        || message.contains("Client error (Connect)")
}

/// The address of an instance as people type it ("fuwa.chat",
/// "localhost:8080/"), the way it's kept: a scheme (https, or http for this
/// machine and local networks unless one was typed), the host in lowercase,
/// and any path without its trailing slash. Like the web app's `normalizeUrl`.
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Type the address of an instance, like fuwa.chat.".into());
    }
    let lower = input.to_ascii_lowercase();
    let text = if lower.starts_with("http://") || lower.starts_with("https://") {
        input.to_owned()
    } else if input.contains("://") {
        return Err("That doesn't look like an instance's address.".into());
    } else {
        let local = ["localhost", "127.", "[::1]", "0.0.0.0", "10.", "192.168."].iter().any(|p| lower.starts_with(p));
        format!("{}://{input}", if local { "http" } else { "https" })
    };
    let url = url::Url::parse(&text).map_err(|_| "That doesn't look like an address.".to_string())?;
    if url.host_str().is_none_or(str::is_empty) {
        return Err("That address has no host.".into());
    }
    let origin = url.origin().ascii_serialization();
    Ok(format!("{origin}{}", url.path().trim_end_matches('/')))
}

/// A short, readable id for an instance: "fuwa.chat", as the web app's `instanceKey`.
pub fn instance_key(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let host = match u.port() {
                Some(port) => format!("{}:{port}", u.host_str().unwrap_or_default()),
                None => u.host_str().unwrap_or_default().to_owned(),
            };
            format!("{host}{}", u.path().trim_end_matches('/')).replace('/', "~")
        }
        Err(_) => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized() {
        assert_eq!(normalize_url("fuwa.chat").unwrap(), "https://fuwa.chat");
        assert_eq!(normalize_url(" https://Fuwa.Chat/ ").unwrap(), "https://fuwa.chat");
        assert_eq!(normalize_url("localhost:8080/").unwrap(), "http://localhost:8080");
        assert_eq!(normalize_url("192.168.1.5:8080").unwrap(), "http://192.168.1.5:8080");
        assert_eq!(normalize_url("chat.example.com:8443/fuwa/").unwrap(), "https://chat.example.com:8443/fuwa");
        assert!(normalize_url("").is_err());
        assert!(normalize_url("ftp://x").is_err());
        assert_eq!(instance_key("https://fuwa.chat"), "fuwa.chat");
        assert_eq!(instance_key("http://localhost:8080"), "localhost:8080");
        assert_eq!(instance_key("https://example.com/a/b"), "example.com~a~b");
    }

    /// A response body that hands out the chunks it's given, then (when
    /// `open`) waits forever, like a live stream between events.
    struct Chunks {
        chunks: std::collections::VecDeque<Bytes>,
        open: bool,
    }

    impl HttpBody for Chunks {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
            match self.chunks.pop_front() {
                Some(chunk) => Poll::Ready(Some(Ok(Frame::data(chunk)))),
                None if self.open => Poll::Pending,
                None => Poll::Ready(None),
            }
        }
    }

    /// A gRPC-Web response through the desktop's stack, as `chunks` on the wire.
    async fn through(chunks: Vec<Vec<u8>>, open: bool) -> http::Response<GrpcWebCall<Frames<Chunks>>> {
        let chunks: std::collections::VecDeque<Bytes> = chunks.into_iter().map(Bytes::from).collect();
        let server = tower::service_fn(move |_: Request<GrpcWebCall<Body>>| {
            let body = Chunks { chunks: chunks.clone(), open };
            async move { Ok::<_, std::convert::Infallible>(http::Response::new(body)) }
        });
        GrpcWebClientLayer::new().layer(Framed(server)).call(Request::new(Body::empty())).await.unwrap()
    }

    fn message(payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![0];
        frame.extend((payload.len() as u32).to_be_bytes());
        frame.extend(payload);
        frame
    }

    fn status(text: &str) -> Vec<u8> {
        let mut frame = vec![0x80];
        frame.extend((text.len() as u32).to_be_bytes());
        frame.extend(text.as_bytes());
        frame
    }

    /// However the response is cut into chunks (the status alone, as an
    /// instance reached directly sends it; everything in one, as Cloudflare
    /// does; anywhere inside a frame), the messages and the status come out
    /// the same.
    #[tokio::test]
    async fn any_chunking_reads_the_same() {
        use http_body_util::BodyExt;
        let messages = [message(&[8, 1]), message(&[]), message(&[8, 2, 16, 3])].concat();
        for (end, code, text) in [
            (status("grpc-status:0\r\n"), "0", None),
            (status("grpc-status:5\r\ngrpc-message:nope\r\n"), "5", Some("nope")),
        ] {
            let wire = [messages.clone(), end].concat();
            let mut cuts: Vec<Vec<Vec<u8>>> = vec![
                vec![wire.clone()],
                vec![messages.clone(), wire[messages.len()..].to_vec()],
                wire.iter().map(|byte| vec![*byte]).collect(),
            ];
            for at in 1..wire.len() {
                cuts.push(vec![wire[..at].to_vec(), wire[at..].to_vec()]);
            }
            for chunks in cuts {
                let shape: Vec<usize> = chunks.iter().map(Vec::len).collect();
                let body = through(chunks, false).await.into_body().collect().await.unwrap();
                let trailers = body.trailers().cloned().unwrap_or_else(|| panic!("no status for chunks {shape:?}"));
                assert_eq!(trailers.get("grpc-status").unwrap(), code, "chunks {shape:?}");
                assert_eq!(trailers.get("grpc-message").map(|v| v.to_str().unwrap()), text, "chunks {shape:?}");
                assert_eq!(body.to_bytes(), messages, "chunks {shape:?}");
            }
        }
        // An answer that's only a status, in the headers: an empty body.
        let body = through(vec![], false).await.into_body().collect().await.unwrap();
        assert!(body.trailers().is_none() && body.to_bytes().is_empty());
    }

    /// A live stream's events come through as they arrive, not when it ends.
    #[tokio::test]
    async fn stream_events_are_not_held_back() {
        use http_body_util::BodyExt;
        let first = message(&[8, 1]);
        let second = message(&[8, 2]);
        // The second event cut in two, its end still on the way.
        let chunks = vec![[first.clone(), second[..3].to_vec()].concat(), second[3..5].to_vec()];
        let mut body = through(chunks, true).await.into_body();
        let wait = Duration::from_secs(1);
        let frame = tokio::time::timeout(wait, body.frame()).await.expect("first event held back");
        assert_eq!(frame.unwrap().unwrap().into_data().unwrap(), first);
        assert!(tokio::time::timeout(Duration::from_millis(50), body.frame()).await.is_err(), "half an event given");
    }
}
