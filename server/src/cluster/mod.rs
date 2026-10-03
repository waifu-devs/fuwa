//! Running one instance as several processes, to scale out.
//!
//! By default one `fuwa` process does everything. With `FUWA_ROLE` it does one
//! part of it instead, and the parts talk over the cluster protocol
//! (`proto/fuwa/cluster/v1`) on a private network:
//!
//! - the **directory** keeps node.db (accounts, sessions, settings, uploaded
//!   pictures) and knows which shard holds each community server;
//! - each **shard** keeps some of the servers' files and their live events;
//! - **gateways** are what clients reach: they hold nothing, serve the web app,
//!   and pass each call to the directory or to the shard holding its server;
//! - **media** parts carry calls' sound (WebRTC, see `rtc.rs`); they hold
//!   nothing, and the shards and directory open and close calls on them.
//!
//! Adding shards spreads servers (and their traffic) across machines; adding
//! gateways spreads connections. Every internal call carries the cluster key.

pub mod calls;
pub mod directory;
pub mod gateway;
pub mod index;
pub mod media;
pub mod shard;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tonic::metadata::{AsciiMetadataValue, MetadataMap};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

use crate::cpb;
use crate::error::{Error, Result};
use crate::id::timestamp;
use crate::node::Account;
use crate::pb;

/// The header every internal call carries the cluster key in.
pub const KEY_HEADER: &str = "x-fuwa-cluster-key";

/// Which part of the instance this process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Everything, in one process (the default).
    All,
    Gateway,
    Directory,
    Shard,
    /// Carries calls' sound (WebRTC); keeps nothing.
    Media,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Gateway => "gateway",
            Self::Directory => "directory",
            Self::Shard => "shard",
            Self::Media => "media",
        }
    }
}

/// How this process fits into a split instance.
#[derive(Clone)]
pub struct ClusterConfig {
    /// FUWA_ROLE: all (default) | gateway | directory | shard | media.
    pub role: Role,
    /// FUWA_CLUSTER_KEY: the shared secret on every internal call. Required
    /// unless the role is all.
    pub key: Option<String>,
    /// FUWA_DIRECTORY_URL: where gateways and shards reach the directory.
    pub directory_url: Option<String>,
    /// FUWA_SHARD_ID: a shard's name. Defaults to one made up on first start
    /// and kept in the data directory, so the name goes where the files go.
    pub shard_id: Option<String>,
    /// FUWA_INTERNAL_URL: where the directory and gateways reach this shard.
    pub internal_url: Option<String>,
    /// How long a call waits for another part that's restarting: [`RIDE_OUT`]
    /// (tests make it shorter).
    pub ride_out: Duration,
}

impl std::fmt::Debug for ClusterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterConfig")
            .field("role", &self.role)
            .field("key", &crate::config::Secret(&self.key))
            .field("directory_url", &self.directory_url)
            .field("shard_id", &self.shard_id)
            .field("internal_url", &self.internal_url)
            .field("ride_out", &self.ride_out)
            .finish()
    }
}

impl ClusterConfig {
    pub fn single() -> Self {
        Self { role: Role::All, key: None, directory_url: None, shard_id: None, internal_url: None, ride_out: RIDE_OUT }
    }

    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> std::result::Result<Self, String> {
        let role = match get("FUWA_ROLE").as_deref().map(str::trim) {
            None | Some("all") => Role::All,
            Some("gateway") => Role::Gateway,
            Some("directory") => Role::Directory,
            Some("shard") => Role::Shard,
            Some("media") => Role::Media,
            Some(other) => {
                return Err(format!("FUWA_ROLE must be all, gateway, directory, shard or media, got {other:?}"));
            }
        };
        if role == Role::All {
            return Ok(Self::single());
        }
        let key = get("FUWA_CLUSTER_KEY").map(|key| key.trim().to_string());
        if key.as_ref().is_none_or(|key| key.len() < 32) {
            return Err(format!(
                "FUWA_CLUSTER_KEY must be set to at least 32 characters (the same on every part, e.g. from \
                 `openssl rand -hex 32`) when FUWA_ROLE is {}",
                role.as_str()
            ));
        }
        if key.as_ref().is_some_and(|key| AsciiMetadataValue::try_from(key.as_str()).is_err()) {
            return Err("FUWA_CLUSTER_KEY must be plain ASCII letters, digits and punctuation".into());
        }
        let url = |name: &str| -> std::result::Result<Option<String>, String> {
            match get(name).map(|url| url.trim().trim_end_matches('/').to_string()) {
                Some(url) if url.starts_with("http://") || url.starts_with("https://") => Ok(Some(url)),
                Some(url) => Err(format!("{name} must be an http(s) URL, got {url:?}")),
                None => Ok(None),
            }
        };
        let directory_url = url("FUWA_DIRECTORY_URL")?;
        if matches!(role, Role::Gateway | Role::Shard) && directory_url.is_none() {
            return Err(format!(
                "FUWA_DIRECTORY_URL must say where the directory is (like http://directory:8080) when FUWA_ROLE is {}",
                role.as_str()
            ));
        }
        let internal_url = url("FUWA_INTERNAL_URL")?;
        if role == Role::Shard && internal_url.is_none() {
            return Err(
                "FUWA_INTERNAL_URL must say where the directory and gateways reach this shard (like http://shard-1:8080)"
                    .into(),
            );
        }
        let shard_id = get("FUWA_SHARD_ID").map(|id| id.trim().to_string());
        if let Some(id) = &shard_id {
            check_shard_id(id).map_err(|err| format!("FUWA_SHARD_ID {err}"))?;
        }
        Ok(Self { role, key, directory_url, shard_id, internal_url, ride_out: RIDE_OUT })
    }

    /// This process is one part of several.
    pub fn is_split(&self) -> bool {
        self.role != Role::All
    }

    /// The cluster key as a header value.
    pub fn key_value(&self) -> Result<AsciiMetadataValue> {
        let key = self.key.as_deref().ok_or_else(|| Error::internal("no cluster key"))?;
        AsciiMetadataValue::try_from(key).map_err(|_| Error::internal("the cluster key isn't a valid header"))
    }
}

fn check_shard_id(id: &str) -> std::result::Result<(), String> {
    let valid = (1..=64).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if valid { Ok(()) } else { Err(format!("must be 1 to 64 of a-z, 0-9, - and _, got {id:?}")) }
}

/// Whether a name is one of the files a server's database is made of, as
/// named in servers/: `<id>.db`, or a file kept beside it. Anything else
/// (and any path) isn't.
pub fn is_server_file(name: &str) -> bool {
    crate::servers::SIDECARS.iter().any(|suffix| {
        name.strip_suffix(suffix)
            .and_then(|db| db.strip_suffix(".db"))
            .is_some_and(|id| crate::id::parse_id("server", id).is_ok_and(|parsed| parsed == id))
    })
}

/// A shard's name: FUWA_SHARD_ID, else the one kept in its data directory
/// (made up on first start).
pub fn shard_id(config: &ClusterConfig, data_path: &Path) -> Result<String> {
    if let Some(id) = &config.shard_id {
        return Ok(id.clone());
    }
    let path = data_path.join("shard-id");
    match std::fs::read_to_string(&path) {
        Ok(id) if check_shard_id(id.trim()).is_ok() => Ok(id.trim().to_string()),
        Ok(_) => Err(Error::internal(format!("{} doesn't hold a shard name", path.display()))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let id = format!("shard-{}", crate::id::new_id().to_ascii_lowercase());
            std::fs::write(&path, format!("{id}\n"))?;
            Ok(id)
        }
        Err(err) => Err(err.into()),
    }
}

/// Answers only calls that carry the cluster key, and health checks, so the
/// parts behind the gateways can't be reached around them.
pub async fn require_key(State(key): State<Arc<str>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let health = path == "/healthz" || path.starts_with("/grpc.health.v1.Health/");
    let given = request.headers().get(KEY_HEADER).map(|value| value.as_bytes()).unwrap_or_default();
    if !health && !crate::auth::constant_time_eq(given, key.as_bytes()) {
        let status = tonic::Status::permission_denied("this is an internal part of a fuwa instance; call a gateway");
        return status.into_http::<axum::body::Body>().into_response();
    }
    next.run(request).await
}

/// A connection to another part: made when first used, kept alive, and made
/// again after it drops.
pub fn channel(url: &str) -> Result<Channel> {
    let endpoint = Endpoint::from_shared(url.to_string())
        .map_err(|err| Error::internal(format!("{url} isn't a URL to call: {err}")))?
        .connect_timeout(Duration::from_secs(5))
        .tcp_nodelay(true)
        .http2_keep_alive_interval(Duration::from_secs(20))
        .keep_alive_timeout(Duration::from_secs(10))
        .keep_alive_while_idle(true);
    Ok(endpoint.connect_lazy())
}

/// Adds the cluster key to every call.
#[derive(Clone)]
pub struct WithKey(pub AsciiMetadataValue);

impl Interceptor for WithKey {
    fn call(&mut self, mut request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        request.metadata_mut().insert(KEY_HEADER, self.0.clone());
        Ok(request)
    }
}

pub type Keyed = InterceptedService<Channel, WithKey>;
pub type DirectoryClient = cpb::directory_service_client::DirectoryServiceClient<Keyed>;
pub type ShardClient = cpb::shard_service_client::ShardServiceClient<Keyed>;

/// Internal messages can be as big as client ones get (a shard's server list,
/// an export piece).
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

pub fn directory_client(url: &str, key: AsciiMetadataValue) -> Result<DirectoryClient> {
    Ok(cpb::directory_service_client::DirectoryServiceClient::with_interceptor(channel(url)?, WithKey(key))
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE))
}

pub fn shard_client(url: &str, key: AsciiMetadataValue) -> Result<ShardClient> {
    Ok(cpb::shard_service_client::ShardServiceClient::with_interceptor(channel(url)?, WithKey(key))
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE))
}

pub fn directory_server<T: cpb::directory_service_server::DirectoryService>(
    service: T,
) -> cpb::directory_service_server::DirectoryServiceServer<T> {
    cpb::directory_service_server::DirectoryServiceServer::new(service)
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE)
}

pub fn shard_server<T: cpb::shard_service_server::ShardService>(
    service: T,
) -> cpb::shard_service_server::ShardServiceServer<T> {
    cpb::shard_service_server::ShardServiceServer::new(service)
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE)
}

/// Waits a little longer after each failure, up to 2 seconds: a part that's
/// back after a restart is noticed soon.
pub struct Backoff(Duration);

impl Default for Backoff {
    fn default() -> Self {
        Self(Duration::from_millis(250))
    }
}

impl Backoff {
    pub fn wait(&mut self) -> Duration {
        let wait = self.0;
        self.0 = (self.0 * 2).min(Duration::from_secs(2));
        wait
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// How long a call waits for a part that's restarting (for a deploy, say)
/// before giving up on it. A part with a volume can't overlap its old and new
/// deployments, so it's gone for a few seconds each time.
pub const RIDE_OUT: Duration = Duration::from_secs(30);

/// Paces tries at a part that's restarting: right away, then a little longer
/// each time up to a second, until the time runs out.
pub struct Patience {
    until: tokio::time::Instant,
    wait: Duration,
}

impl Patience {
    pub fn new(within: Duration) -> Self {
        Self { until: tokio::time::Instant::now() + within, wait: Duration::ZERO }
    }

    /// Waits before the next try; false once the time is up.
    pub async fn wait(&mut self) -> bool {
        let now = tokio::time::Instant::now();
        if now >= self.until {
            return false;
        }
        tokio::time::sleep(self.wait.min(self.until - now)).await;
        self.wait = (self.wait * 2).clamp(Duration::from_millis(100), Duration::from_secs(1));
        true
    }
}

/// Whether a call failed because the part it went to is down or restarting.
pub fn unreachable(status: &tonic::Status) -> bool {
    status.code() == tonic::Code::Unavailable || std::error::Error::source(status).is_some()
}

/// Makes a call that only reads, trying it again while the part it goes to is
/// unreachable, for up to `within`. Only giving up is logged, as a warning.
pub async fn ride_out<T, F>(within: Duration, mut call: impl FnMut() -> F) -> std::result::Result<T, tonic::Status>
where
    F: Future<Output = std::result::Result<T, tonic::Status>>,
{
    let mut patience = Patience::new(within);
    loop {
        match call().await {
            Err(status) if unreachable(&status) => {
                if !patience.wait().await {
                    tracing::warn!(error = %status, waited = ?within, "a part of this instance didn't answer in time");
                    return Err(Error::retried(status).into());
                }
            }
            answer => return answer,
        }
    }
}

/// Copies the headers a call is made with (who's calling, from what) onto
/// a call to another part.
pub fn forward_metadata<T>(from: &MetadataMap, message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    for name in ["authorization", "user-agent"] {
        if let Some(value) = from.get(name) {
            request.metadata_mut().insert(name, value.clone());
        }
    }
    request
}

pub fn account_to_pb(account: &Account) -> cpb::Account {
    cpb::Account {
        id: account.id.clone(),
        kind: account.kind as i32,
        username: account.username.clone(),
        display_name: account.display_name.clone(),
        avatar_url: account.avatar_url.clone(),
        admin: account.admin,
        created_at: Some(timestamp(account.created_at)),
        last_seen_at: Some(timestamp(account.last_seen_at)),
        status: account.status.clone(),
        status_expires_at: account.status_expires_at.map(timestamp),
        two_factor: account.two_factor,
        disabled: account.disabled,
        has_password: account.has_password(),
    }
}

pub fn account_from_pb(account: cpb::Account) -> Account {
    let millis = |t: Option<prost_types::Timestamp>| t.as_ref().map(crate::id::millis);
    Account {
        id: account.id,
        kind: pb::AccountKind::try_from(account.kind).unwrap_or(pb::AccountKind::Unspecified),
        username: account.username,
        display_name: account.display_name,
        avatar_url: account.avatar_url,
        admin: account.admin,
        created_at: millis(account.created_at).unwrap_or_default(),
        last_seen_at: millis(account.last_seen_at).unwrap_or_default(),
        status: account.status,
        status_expires_at: millis(account.status_expires_at),
        two_factor: account.two_factor,
        disabled: account.disabled,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> std::result::Result<ClusterConfig, String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        ClusterConfig::from_lookup(&|key| vars.get(key).cloned())
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn server_files_by_name() {
        let id = "01J9Z3K8X2V5W7Q4R6T8Y0B2C4";
        assert!(is_server_file(&format!("{id}.db")));
        assert!(is_server_file(&format!("{id}.db-log")));
        assert!(is_server_file(&format!("{id}.db-wal")));
        for name in [
            "node.db".to_string(),
            format!("{id}.db-journal"),
            format!("{}.db", id.to_lowercase()),
            format!("../{id}.db"),
            format!("x/{id}.db"),
            format!("{id}.txt"),
            ".db".to_string(),
        ] {
            assert!(!is_server_file(&name), "{name}");
        }
    }

    #[test]
    fn one_process_by_default() {
        assert_eq!(config(&[]).unwrap().role, Role::All);
        assert!(!config(&[("FUWA_ROLE", "all")]).unwrap().is_split());
    }

    #[test]
    fn parts_need_a_key_and_each_other() {
        assert!(config(&[("FUWA_ROLE", "directory")]).unwrap_err().contains("FUWA_CLUSTER_KEY"));
        assert!(config(&[("FUWA_ROLE", "directory"), ("FUWA_CLUSTER_KEY", "short")]).is_err());
        assert_eq!(config(&[("FUWA_ROLE", "directory"), ("FUWA_CLUSTER_KEY", KEY)]).unwrap().role, Role::Directory);
        let gateway = [("FUWA_ROLE", "gateway"), ("FUWA_CLUSTER_KEY", KEY)];
        assert!(config(&gateway).unwrap_err().contains("FUWA_DIRECTORY_URL"));
        let shard =
            [("FUWA_ROLE", "shard"), ("FUWA_CLUSTER_KEY", KEY), ("FUWA_DIRECTORY_URL", "http://directory:8080/")];
        assert!(config(&shard).unwrap_err().contains("FUWA_INTERNAL_URL"));
        let shard = [shard.as_slice(), &[("FUWA_INTERNAL_URL", "http://shard-1:8080")]].concat();
        let parsed = config(&shard).unwrap();
        assert_eq!(parsed.directory_url.as_deref(), Some("http://directory:8080"));
        assert!(config(&[shard.as_slice(), &[("FUWA_SHARD_ID", "Shard One")]].concat()).is_err());
        assert!(config(&[("FUWA_ROLE", "everything")]).unwrap_err().contains("FUWA_ROLE"));
    }

    #[test]
    fn a_shard_keeps_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = ClusterConfig { role: Role::Shard, ..ClusterConfig::single() };
        let first = shard_id(&config, dir.path()).unwrap();
        assert!(first.starts_with("shard-"));
        assert_eq!(shard_id(&config, dir.path()).unwrap(), first);
        let named = ClusterConfig { shard_id: Some("eu-1".into()), ..config };
        assert_eq!(shard_id(&named, dir.path()).unwrap(), "eu-1");
    }

    #[tokio::test]
    async fn riding_out_tries_again_then_gives_up_as_unavailable() {
        let refused = || tonic::Status::from_error(Box::new(std::io::Error::other("connection refused")));
        let mut tries = 0;
        let answer = ride_out(Duration::from_millis(250), || {
            tries += 1;
            let answer = if tries < 3 { Err(refused()) } else { Ok(tries) };
            async move { answer }
        })
        .await;
        assert_eq!(answer.unwrap(), 3);

        let answer: std::result::Result<(), _> = ride_out(Duration::ZERO, || async { Err(refused()) }).await;
        let status = answer.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unavailable);
        assert_eq!(status.message(), crate::error::UNREACHABLE);
        // Already logged: passing it on doesn't warn again.
        assert!(std::error::Error::source(&status).is_none());
        assert!(matches!(Error::from(status), Error::Remote(_)));
    }
}
