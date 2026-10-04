//! The instance as an MCP server (docs/mcp.md): agents, and the AI apps they
//! run in, use it through the Model Context Protocol at `/mcp`.
//!
//! Stateless Streamable HTTP: every request is one JSON-RPC message in a
//! POST, answered with plain JSON, and nothing is kept between requests (no
//! `Mcp-Session-Id`, no streams), so any gateway can answer any of them. The
//! agent signs in with its token as a bearer token, as with the gRPC API.
//!
//! Every tool is one or two calls to the public gRPC API, made inside this
//! process through the same router clients reach (`inner`): the gateway's
//! on a split instance, the whole instance's on a single process. So each
//! one is routed, permission-checked, limited and timed exactly as the call
//! it wraps, and this module never touches a database.

mod catalog;
mod tools;
mod view;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tonic::metadata::AsciiMetadataValue;

use crate::app::HasSettings;
use crate::pb;

/// The protocol versions this endpoint speaks, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

/// Where the endpoint is, on the instance's public address.
pub const PATH: &str = "/mcp";

/// The SDK for programs that want live events and everything else the API does.
pub const SDK: &str = "@waifu-devs/fuwa";

/// The largest request: a picture upload carries its bytes as base64.
pub(crate) const MAX_BODY: usize = 12 * 1024 * 1024;

/// Requests one agent may make, per gateway (or single process): a burst of
/// `BURST`, refilled at `PER_MINUTE` a minute.
const BURST: f64 = 60.0;
const PER_MINUTE: f64 = 120.0;

pub struct Mcp {
    /// The instance's public routes, for the calls each tool makes.
    inner: Router,
    settings: Arc<dyn HasSettings>,
    limits: Limits,
}

/// The MCP endpoint and its discovery document, answering through `inner`.
pub fn routes(inner: Router, settings: Arc<dyn HasSettings>) -> Router {
    let mcp = Arc::new(Mcp { inner, settings, limits: Limits::default() });
    Router::new()
        .route(PATH, post(answer).get(no_stream).delete(no_stream))
        .route("/.well-known/mcp.json", get(card))
        .with_state(mcp)
}

/// One request's agent and how to call the API as it.
pub(crate) struct Cx {
    inner: Router,
    authorization: AsciiMetadataValue,
    me: pb::User,
    instance: String,
}

impl Cx {
    /// A gRPC request carrying the agent's token.
    fn request<T>(&self, message: T) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        request.metadata_mut().insert("authorization", self.authorization.clone());
        request
    }
}

/// Calls a gRPC method through the instance's own router, as the agent.
macro_rules! call {
    ($cx:expr, $client:ident :: $type:ident . $method:ident ( $message:expr )) => {
        crate::pb::$client::$type::new($cx.inner.clone())
            .$method($cx.request($message))
            .await
            .map(tonic::Response::into_inner)
    };
}
pub(crate) use call;

/// A JSON-RPC error.
pub(crate) struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self { code: -32602, message: message.into() }
    }

    fn method(method: &str) -> Self {
        Self { code: -32601, message: format!("there's no method {method:?}") }
    }
}

/// GET opens no stream here (the endpoint is stateless), and there's no session to DELETE.
async fn no_stream() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")], "POST JSON-RPC messages here").into_response()
}

/// What an agent (or a person setting one up) reads to find the endpoint.
async fn card(State(mcp): State<Arc<Mcp>>) -> Response {
    let settings = mcp.settings.settings();
    if !settings.mcp {
        return (StatusCode::NOT_FOUND, "MCP is off on this instance").into_response();
    }
    let body = json!({
        "name": "fuwa",
        "title": settings.name,
        "version": crate::VERSION,
        "description": "A fuwa chat instance. Agents read and write in the servers they were added to.",
        "endpoint": format!("{}{PATH}", settings.public_url.trim_end_matches('/')),
        "transport": { "type": "streamable-http", "stateless": true },
        "protocolVersions": PROTOCOL_VERSIONS,
        "authentication": {
            "required": true,
            "schemes": ["bearer"],
            "description": "An agent's token (Settings > Agents in a fuwa app)."
        },
        "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
        "sdk": { "npm": SDK },
        "documentation": "https://github.com/waifu-devs/fuwa/blob/master/docs/mcp.md"
    });
    let mut response = json_response(&body);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
    response
}

async fn answer(State(mcp): State<Arc<Mcp>>, headers: HeaderMap, body: Body) -> Response {
    let started = Instant::now();
    let response = answer_inner(&mcp, &headers, body).await;
    crate::reports::server_timing("mcp.request", started.elapsed());
    response
}

async fn answer_inner(mcp: &Mcp, headers: &HeaderMap, body: Body) -> Response {
    let settings = mcp.settings.settings();
    if !settings.mcp {
        return plain(StatusCode::NOT_FOUND, "MCP is off on this instance");
    }
    // Browsers on other sites only with the instance's say-so (DNS rebinding).
    if let Some(origin) = headers.get(header::ORIGIN)
        && !settings.allows_origin(origin.as_bytes())
        && !same_origin(origin, &settings.public_url)
    {
        return plain(StatusCode::FORBIDDEN, "this origin may not use the instance");
    }
    if let Some(version) = headers.get("mcp-protocol-version")
        && !PROTOCOL_VERSIONS.iter().any(|known| version.as_bytes() == known.as_bytes())
    {
        return plain(StatusCode::BAD_REQUEST, "unsupported MCP-Protocol-Version");
    }

    let Some(authorization) = bearer(headers) else {
        return unauthorized("an agent's token is needed, as a bearer token");
    };
    let mut cx =
        Cx { inner: mcp.inner.clone(), authorization, me: pb::User::default(), instance: settings.name.clone() };
    match call!(cx, auth_service_client::AuthServiceClient.get_me(pb::GetMeRequest {})) {
        Ok(me) => cx.me = me.user.unwrap_or_default(),
        Err(status) if status.code() == tonic::Code::Unauthenticated => {
            return unauthorized("that token isn't signed in here");
        }
        Err(status)
            if matches!(status.code(), tonic::Code::Unavailable | tonic::Code::Internal | tonic::Code::Unknown) =>
        {
            return plain(StatusCode::SERVICE_UNAVAILABLE, "the instance couldn't check the token; try again");
        }
        // The operator's token, say: no account behind it.
        Err(_) => {}
    }
    if cx.me.kind != pb::AccountKind::Agent as i32 {
        return plain(StatusCode::FORBIDDEN, "only agents use MCP here: make one in Settings > Agents");
    }
    if let Err(wait) = mcp.limits.take(&cx.me.id) {
        crate::reports::server_used("mcp.limited", 1);
        let retry = wait.as_secs().max(1).to_string();
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry)],
            "too many requests from this agent; slow down",
        )
            .into_response();
    }

    // Read only once the token and the limit say yes.
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE, "that request is too big");
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => return rpc(Value::Null, Err(RpcError { code: -32700, message: "that isn't JSON".into() })),
    };
    let Value::Object(message) = message else {
        // Batches went away in the 2025-06-18 protocol.
        return rpc(
            Value::Null,
            Err(RpcError { code: -32600, message: "send one JSON-RPC message per request".into() }),
        );
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response to something we asked: we never ask anything.
        return StatusCode::ACCEPTED.into_response();
    };
    let Some(id) = message.get("id").cloned().filter(|id| id.is_string() || id.is_number()) else {
        // Notifications (initialized, cancelled) need nothing back.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    rpc(id, dispatch(&cx, method, &params).await)
}

async fn dispatch(cx: &Cx, method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize(cx, params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list() })),
        "tools/call" => tools::call(cx, params).await,
        "resources/list" => catalog::list_resources(cx).await,
        "resources/templates/list" => Ok(json!({ "resourceTemplates": catalog::templates() })),
        "resources/read" => catalog::read_resource(cx, params).await,
        "prompts/list" => Ok(json!({ "prompts": catalog::prompts() })),
        "prompts/get" => catalog::get_prompt(cx, params),
        _ => Err(RpcError::method(method)),
    }
}

fn initialize(cx: &Cx, params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
    let version = PROTOCOL_VERSIONS.iter().find(|known| **known == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "listChanged": false, "subscribe": false },
            "prompts": { "listChanged": false }
        },
        "serverInfo": { "name": "fuwa", "title": cx.instance, "version": crate::VERSION },
        "instructions": format!(
            "You are the agent @{} on the fuwa chat instance {:?}. Servers are communities you were added to; \
             their channels hold messages. Start with list_servers and list_channels. Mention someone as <@user id> \
             and a role as <@&role id>. To follow what happens, call list_events without a cursor once, then again \
             with the cursor it returns. Every tool does only what your roles in that server allow.",
            cx.me.username, cx.instance
        )
    })
}

fn rpc(id: Value, result: Result<Value, RpcError>) -> Response {
    let body = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": err.code, "message": err.message } }),
    };
    json_response(&body)
}

fn json_response(body: &Value) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

fn unauthorized(message: &'static str) -> Response {
    (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"fuwa\""))], message)
        .into_response()
}

/// The request's bearer token as gRPC metadata, never looked at here.
fn bearer(headers: &HeaderMap) -> Option<AsciiMetadataValue> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    format!("Bearer {token}").parse().ok()
}

/// Whether `origin` is the instance's own address.
fn same_origin(origin: &HeaderValue, public_url: &str) -> bool {
    let Ok(url) = url::Url::parse(public_url) else { return false };
    origin.as_bytes() == url.origin().ascii_serialization().as_bytes()
}

/// Each agent's requests, as token buckets kept by account id.
#[derive(Default)]
struct Limits {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limits {
    /// Takes one request from the agent's bucket, or says how long to wait.
    fn take(&self, account_id: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if buckets.len() > 10_000 {
            // Agents that have been quiet long enough to be full again.
            let full = Duration::from_secs_f64(BURST / PER_MINUTE * 60.0);
            buckets.retain(|_, (_, at)| now.duration_since(*at) < full);
        }
        let (tokens, at) = buckets.entry(account_id.to_string()).or_insert((BURST, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * PER_MINUTE / 60.0).min(BURST);
        *at = now;
        if *tokens < 1.0 {
            return Err(Duration::from_secs_f64((1.0 - *tokens) * 60.0 / PER_MINUTE));
        }
        *tokens -= 1.0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_agent_on_its_own() {
        let limits = Limits::default();
        for _ in 0..BURST as usize {
            assert!(limits.take("a").is_ok());
        }
        assert!(limits.take("a").is_err());
        assert!(limits.take("b").is_ok());
    }

    #[test]
    fn reads_bearer_tokens() {
        let mut headers = HeaderMap::new();
        assert!(bearer(&headers).is_none());
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert!(bearer(&headers).is_none());
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("bearer  abc "));
        assert_eq!(bearer(&headers).unwrap().to_str().unwrap(), "Bearer abc");
    }

    #[test]
    fn knows_its_own_origin() {
        assert!(same_origin(&HeaderValue::from_static("https://chat.example.com"), "https://chat.example.com/"));
        assert!(!same_origin(&HeaderValue::from_static("https://evil.example"), "https://chat.example.com"));
    }
}
