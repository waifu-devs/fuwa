//! Instance configuration, from `FUWA_*` environment variables. A `.env` file in
//! the working directory is read first; real environment variables win over it.

use std::env;
use std::path::PathBuf;

use crate::cluster::{ClusterConfig, Role};
use crate::db::EncryptionKey;
use crate::pb;
use crate::replica::{ReplicaConfig, Restore, S3Config, Target};

#[derive(Clone)]
pub struct Config {
    /// Where node.db and the servers/ directory live. FUWA_DATA_PATH, default ~/.fuwa.
    pub data_path: PathBuf,
    /// FUWA_HOST, default 0.0.0.0. `::` listens on IPv6 and IPv4 both.
    pub host: String,
    /// FUWA_PORT, else PORT (as Railway and similar hosts set it), default 8080.
    pub port: u16,
    /// The URL clients reach this instance on. FUWA_PUBLIC_URL, default http://localhost:<port>.
    pub public_url: String,
    /// FUWA_NODE_NAME, default "Fuwa".
    pub node_name: String,
    /// FUWA_ALLOWED_ORIGINS: `*` (default) or a comma-separated list of browser
    /// origins allowed to call the API. Sessions travel in headers, not cookies,
    /// so `*` is safe.
    pub allowed_origins: Vec<String>,
    /// FUWA_ENCRYPTION_KEY: when set, every database is encrypted at rest.
    pub encryption_key: Option<EncryptionKey>,
    /// FUWA_LOCAL_ACCOUNTS: open (default) | closed | off.
    pub local_accounts: Accounts,
    /// FUWA_LINKED_ACCOUNTS: open (default) | closed | off. Accounts signed in
    /// with waifu.dev, which needs a public URL waifu.dev can send people back to.
    pub linked_accounts: Accounts,
    /// FUWA_LINKED_ISSUER, default https://api.waifu.dev: the OpenAuth issuer
    /// linked accounts sign in with.
    pub linked_issuer: String,
    /// FUWA_SSO_ACCOUNTS: open | closed | off (default). Signing in through
    /// the identity provider admins set up in the app (single sign-on).
    pub sso_accounts: Accounts,
    /// FUWA_SERVER_CREATION: everyone (default) | admins | off.
    pub server_creation: pb::ServerCreation,
    /// FUWA_AGENT_CREATION: who may make agents (bot accounts): everyone
    /// (default) | admins | off.
    pub agent_creation: pb::AgentCreation,
    /// FUWA_ADMIN_TOKEN: a bearer token with instance-admin rights, for a control
    /// plane or scripts. Unset means only admin accounts are admins.
    pub admin_token: Option<String>,
    /// FUWA_LIMIT_*: caps every server gets unless it has its own, and on
    /// picture uploads. Unlimited by default, but for picture uploads.
    pub limits: Limits,
    pub telemetry: Telemetry,
    /// FUWA_WEB: on (default) | off. Serves the web client on / when the binary
    /// was built with it (the Docker image and release builds are).
    pub web: bool,
    /// FUWA_ROLE and the rest of how a split instance fits together.
    pub cluster: ClusterConfig,
    /// FUWA_CALLS: on (default) | off. Voice channels and direct-message calls.
    pub calls: bool,
    /// FUWA_CALL_RECORDINGS: on (default) | off. Recording voice channels on
    /// the server, for people with RECORD.
    pub call_recordings: bool,
    /// FUWA_SHARED_CHANNELS: on (default) | off. Servers sharing channels
    /// with each other.
    pub shared_channels: bool,
    /// FUWA_MCP: on (default) | off. Agents using the instance through MCP
    /// at /mcp (docs/mcp.md).
    pub mcp: bool,
    /// People may put an effect on their profile card. FUWA_PROFILE_EFFECTS.
    pub profile_effects: bool,
    /// FUWA_FEDERATION: on | off (default). Sharing channels with servers on
    /// other fuwa instances (docs/federation.md).
    pub federation: bool,
    /// FUWA_FEDERATION_ALLOW_PRIVATE: lets this instance talk to other
    /// instances on private, loopback or internal addresses, and over plain
    /// http. Off by default; for tests and private deployments. Env only.
    pub federation_allow_private: bool,
    /// FUWA_CALL_RECORDINGS_KEEP_DAYS: days a finished server recording is
    /// kept before it deletes itself. Unset (default): until someone does.
    pub call_recordings_keep_days: Option<i64>,
    /// FUWA_ICE_URLS: STUN and TURN servers apps reach the media part
    /// through (comma-separated stun:, turn: and turns: URLs). None by default.
    pub ice_urls: Vec<String>,
    /// FUWA_TURN_SECRET: the TURN servers' shared secret (coturn's
    /// static-auth-secret), to make each caller a credential.
    pub turn_secret: String,
    /// Moderation providers servers' AutoMod can use, on from the start:
    /// FUWA_JEV_API_KEY turns TypeSafe Jev on; FUWA_CLEF_API_TOKEN and
    /// FUWA_CLEF_ACCOUNT_ID turn Cloudflare Clef on. None by default.
    pub automod_providers: Vec<crate::automod::providers::Setup>,
    /// GIF search: FUWA_GIF_PROVIDER (giphy or klipy) with FUWA_GIF_API_KEY
    /// turns it on from the start. Off by default.
    pub gifs: crate::gifs::Setup,
    /// FUWA_GIF_API_URL: where the GIF provider's API is instead of its own
    /// address, reached without the public-address check. For tests only.
    pub gif_api_url: Option<String>,
    /// Where this process carries calls itself (one process, or a media
    /// part): FUWA_MEDIA_PORT (default 50000, UDP and TCP; `off` for no
    /// calls here) and FUWA_MEDIA_ADDRESSES. None when it's off, or this
    /// part doesn't carry calls.
    pub media: Option<crate::rtc::MediaConfig>,
    /// FUWA_MEDIA_URL: on a split instance's directory and shards, where the
    /// media parts are (comma-separated internal URLs).
    pub media_urls: Vec<String>,
    /// Where the databases and pictures are continuously copied to:
    /// FUWA_S3_* (a bucket) or FUWA_REPLICA_PATH (a directory). None is off.
    pub replica: Option<ReplicaConfig>,
    /// FUWA_STREAMS_PER_ACCOUNT: live connections (apps and tabs) one account
    /// may hold open at once on this part, default 32 (a protective default);
    /// `unlimited` for none. Admins can change it in the instance settings.
    pub streams_per_account: Option<usize>,
    /// FUWA_MAX_STREAMS: live connections this part holds open at once, for
    /// everyone. Unlimited by default; see docs/capacity.md for what a box holds.
    pub max_streams: Option<usize>,
    /// FUWA_WRITE_QUEUE: writes one server's file may have waiting at once
    /// before that server answers busy, default 512 (a protective default);
    /// `unlimited` for none.
    pub write_queue: Option<usize>,
    /// FUWA_SIGN_IN_QUEUE: password checks that may wait at once before
    /// sign-ins answer busy, default 256 (a protective default); `unlimited`
    /// for none.
    pub sign_in_queue: Option<usize>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("data_path", &self.data_path)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("public_url", &self.public_url)
            .field("node_name", &self.node_name)
            .field("allowed_origins", &self.allowed_origins)
            .field("encryption_key", &self.encryption_key)
            .field("local_accounts", &self.local_accounts)
            .field("linked_accounts", &self.linked_accounts)
            .field("linked_issuer", &self.linked_issuer)
            .field("server_creation", &self.server_creation)
            .field("agent_creation", &self.agent_creation)
            .field("admin_token", &Secret(&self.admin_token))
            .field("limits", &self.limits)
            .field("telemetry", &self.telemetry)
            .field("web", &self.web)
            .field("cluster", &self.cluster)
            .field("replica", &self.replica)
            .field("streams_per_account", &self.streams_per_account)
            .field("max_streams", &self.max_streams)
            .field("write_queue", &self.write_queue)
            .field("sign_in_queue", &self.sign_in_queue)
            .field("automod_providers", &self.automod_providers)
            .field("gifs", &self.gifs)
            .finish()
    }
}

/// A secret that may be set, for `Debug`: whether it is, never what it is.
pub struct Secret<'a>(pub &'a Option<String>);

impl std::fmt::Debug for Secret<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Some(***)" } else { "None" })
    }
}

/// Whether a kind of account works here: standalone ones (username and
/// password, kept on this instance) or linked ones (signed in with waifu.dev).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accounts {
    /// Anyone can sign up.
    Open,
    /// Existing accounts can sign in; nobody new can sign up.
    Closed,
    /// None of this kind at all.
    Off,
}

impl Accounts {
    pub fn sign_in(self) -> bool {
        self != Self::Off
    }

    pub fn sign_up(self) -> bool {
        self == Self::Open
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

pub const DEFAULT_LINKED_ISSUER: &str = "https://api.waifu.dev";

/// Instance-wide caps. `None` is unlimited, and everything is unlimited by
/// default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    /// FUWA_LIMIT_SERVERS_PER_ACCOUNT: servers one account may own.
    pub servers_per_account: Option<i64>,
    /// FUWA_LIMIT_MEMBERS: members per server.
    pub members: Option<i64>,
    /// FUWA_LIMIT_CHANNELS: channels per server.
    pub channels: Option<i64>,
    /// FUWA_LIMIT_STORAGE: database size per server, e.g. `500MB` or `2GiB`.
    pub storage_bytes: Option<i64>,
    /// FUWA_LIMIT_ATTACHMENT_STORAGE: uploaded files per server, e.g. `10GB`.
    pub attachment_bytes: Option<i64>,
    /// FUWA_LIMIT_EMOJIS: custom emoji per server.
    pub emojis: Option<i64>,
    /// FUWA_LIMIT_RECORDING_STORAGE: voice channel recordings kept on the
    /// server, per server, e.g. `20GB`.
    pub recording_bytes: Option<i64>,
    /// FUWA_LIMIT_PICTURE_UPLOAD: the largest avatar, banner or server icon one
    /// upload may be, e.g. `8MiB`.
    pub picture_upload_bytes: Option<i64>,
    /// FUWA_LIMIT_PICTURE_UPLOADS_PER_DAY: how many bytes of pictures one
    /// account may upload in a day (UTC), e.g. `256MiB`.
    pub picture_upload_bytes_per_day: Option<i64>,
    /// FUWA_LIMIT_ATTACHMENT_UPLOAD: the largest file one attachment upload
    /// may be, e.g. `100MB`.
    pub attachment_upload_bytes: Option<i64>,
    /// FUWA_LIMIT_ATTACHMENT_UPLOADS_PER_DAY: how many bytes of attachments
    /// one account may upload in a day (UTC), e.g. `2GiB`.
    pub attachment_upload_bytes_per_day: Option<i64>,
    /// FUWA_LIMIT_AUTOMOD_CHECKS_PER_DAY: how many times a day (UTC) one
    /// server's Smart filter may ask its provider.
    pub automod_checks_per_day: Option<i64>,
    /// FUWA_LIMIT_SHARED_REMOTE_SENDS_PER_MINUTE: messages a minute all the
    /// people of one server on another instance may send together.
    pub shared_remote_sends_per_minute: Option<i64>,
    /// FUWA_LIMIT_SHARED_REMOTE_PEOPLE: people one server on another
    /// instance may bring to a server's shared channels.
    pub shared_remote_people: Option<i64>,
}

impl Limits {
    /// Whether any cap differs from the defaults.
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

/// The anonymous usage signal. See README.md for exactly what it contains.
#[derive(Debug, Clone)]
pub struct Telemetry {
    /// FUWA_TELEMETRY: on (default) | off. DO_NOT_TRACK=1 also turns it off.
    pub enabled: bool,
    /// FUWA_TELEMETRY_URL, default https://analytics.waifu.dev/v1/fuwa/signals.
    pub url: String,
    /// FUWA_REPORTS_URL, where the hourly health report goes. Defaults to
    /// FUWA_TELEMETRY_URL with `/signals` changed to `/reports`, so
    /// https://analytics.waifu.dev/v1/fuwa/reports.
    pub reports_url: String,
    /// FUWA_HOSTING: self_hosted (default) | hosted. Only Waifu Devs' own hosted
    /// instance says hosted, so the signal can tell the two apart.
    pub hosted: bool,
}

pub const DEFAULT_TELEMETRY_URL: &str = "https://analytics.waifu.dev/v1/fuwa/signals";
pub const DEFAULT_REPORTS_URL: &str = "https://analytics.waifu.dev/v1/fuwa/reports";

impl Config {
    /// Reads the configuration from the environment (and `.env`).
    pub fn load() -> Result<Self, String> {
        let _ = dotenvy::dotenv();
        Self::from_lookup(|key| env::var(key).ok().filter(|value| !value.trim().is_empty()))
    }

    /// Builds a configuration from any source of variables.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let data_path = match get("FUWA_DATA_PATH") {
            Some(path) => PathBuf::from(path),
            None => env::home_dir().map(|home| home.join(".fuwa")).unwrap_or_else(|| PathBuf::from(".fuwa")),
        };

        let port = match get("FUWA_PORT").or_else(|| get("PORT")) {
            Some(port) => port
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("FUWA_PORT must be a port number between 1 and 65535, got {port:?}"))?,
            None => 8080,
        };

        let public_url = get("FUWA_PUBLIC_URL")
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("http://localhost:{port}"));
        if !(public_url.starts_with("http://") || public_url.starts_with("https://")) {
            return Err(format!("FUWA_PUBLIC_URL must start with http:// or https://, got {public_url:?}"));
        }

        let allowed_origins = get("FUWA_ALLOWED_ORIGINS")
            .unwrap_or_else(|| "*".into())
            .split(',')
            .map(|origin| origin.trim().trim_end_matches('/').to_string())
            .filter(|origin| !origin.is_empty())
            .collect();

        let encryption_key = get("FUWA_ENCRYPTION_KEY")
            .map(|key| EncryptionKey::parse(&key).map_err(|err| format!("FUWA_ENCRYPTION_KEY {err}")))
            .transpose()?;

        let accounts = |key: &str| match get(key) {
            None => Ok(Accounts::Open),
            Some(value) => Accounts::parse(value.trim())
                .ok_or_else(|| format!("{key} must be open, closed or off, got {:?}", value.trim())),
        };
        let local_accounts = accounts("FUWA_LOCAL_ACCOUNTS")?;
        let linked_accounts = accounts("FUWA_LINKED_ACCOUNTS")?;
        let sso_accounts = match get("FUWA_SSO_ACCOUNTS") {
            None => Accounts::Off,
            Some(_) => accounts("FUWA_SSO_ACCOUNTS")?,
        };
        let linked_issuer = get("FUWA_LINKED_ISSUER")
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_LINKED_ISSUER.into());
        if !(linked_issuer.starts_with("https://") || linked_issuer.starts_with("http://")) {
            return Err(format!("FUWA_LINKED_ISSUER must start with https://, got {linked_issuer:?}"));
        }

        let server_creation = match get("FUWA_SERVER_CREATION").as_deref().map(str::trim) {
            None | Some("everyone") => pb::ServerCreation::Everyone,
            Some("admins") => pb::ServerCreation::Admins,
            Some("off") => pb::ServerCreation::Disabled,
            Some(other) => {
                return Err(format!("FUWA_SERVER_CREATION must be everyone, admins or off, got {other:?}"));
            }
        };

        let agent_creation = match get("FUWA_AGENT_CREATION").as_deref().map(str::trim) {
            None | Some("everyone") => pb::AgentCreation::Everyone,
            Some("admins") => pb::AgentCreation::Admins,
            Some("off") => pb::AgentCreation::Disabled,
            Some(other) => {
                return Err(format!("FUWA_AGENT_CREATION must be everyone, admins or off, got {other:?}"));
            }
        };

        let admin_token = get("FUWA_ADMIN_TOKEN").map(|token| token.trim().to_string());
        if let Some(token) = &admin_token
            && token.len() < 32
        {
            return Err("FUWA_ADMIN_TOKEN must be at least 32 characters, e.g. from `openssl rand -hex 32`".into());
        }

        let count = |key: &str| -> Result<Option<i64>, String> {
            get(key)
                .map(|value| {
                    value
                        .trim()
                        .parse::<i64>()
                        .ok()
                        .filter(|n| *n >= 0)
                        .ok_or_else(|| format!("{key} must be a whole number of 0 or more, got {value:?}"))
                })
                .transpose()
        };
        let bytes = |key: &str| -> Result<Option<i64>, String> {
            get(key).map(|value| parse_bytes(&value).map_err(|err| format!("{key} {err}"))).transpose()
        };
        // Upload caps also take `unlimited`, the same as leaving them unset.
        let upload_bytes = |key: &str| -> Result<Option<i64>, String> {
            match get(key) {
                Some(value) if value.trim().eq_ignore_ascii_case("unlimited") => Ok(None),
                _ => bytes(key),
            }
        };
        let limits = Limits {
            servers_per_account: count("FUWA_LIMIT_SERVERS_PER_ACCOUNT")?,
            members: count("FUWA_LIMIT_MEMBERS")?,
            channels: count("FUWA_LIMIT_CHANNELS")?,
            storage_bytes: bytes("FUWA_LIMIT_STORAGE")?,
            attachment_bytes: bytes("FUWA_LIMIT_ATTACHMENT_STORAGE")?,
            emojis: count("FUWA_LIMIT_EMOJIS")?,
            recording_bytes: bytes("FUWA_LIMIT_RECORDING_STORAGE")?,
            picture_upload_bytes: upload_bytes("FUWA_LIMIT_PICTURE_UPLOAD")?,
            picture_upload_bytes_per_day: upload_bytes("FUWA_LIMIT_PICTURE_UPLOADS_PER_DAY")?,
            attachment_upload_bytes: upload_bytes("FUWA_LIMIT_ATTACHMENT_UPLOAD")?,
            attachment_upload_bytes_per_day: upload_bytes("FUWA_LIMIT_ATTACHMENT_UPLOADS_PER_DAY")?,
            automod_checks_per_day: count("FUWA_LIMIT_AUTOMOD_CHECKS_PER_DAY")?,
            shared_remote_sends_per_minute: count("FUWA_LIMIT_SHARED_REMOTE_SENDS_PER_MINUTE")?,
            shared_remote_people: count("FUWA_LIMIT_SHARED_REMOTE_PEOPLE")?,
        };

        let do_not_track = get("DO_NOT_TRACK").is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"));
        let telemetry_enabled = match get("FUWA_TELEMETRY").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => !do_not_track,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_TELEMETRY must be on or off, got {other:?}")),
        };

        let hosted = match get("FUWA_HOSTING").as_deref().map(str::trim) {
            None | Some("self_hosted") => false,
            Some("hosted") => true,
            Some(other) => return Err(format!("FUWA_HOSTING must be self_hosted or hosted, got {other:?}")),
        };

        let web = match get("FUWA_WEB").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => true,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_WEB must be on or off, got {other:?}")),
        };

        let cluster = ClusterConfig::from_lookup(&get)?;

        let calls = match get("FUWA_CALLS").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => true,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_CALLS must be on or off, got {other:?}")),
        };
        let call_recordings = match get("FUWA_CALL_RECORDINGS").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => true,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_CALL_RECORDINGS must be on or off, got {other:?}")),
        };
        let shared_channels = match get("FUWA_SHARED_CHANNELS").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => true,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_SHARED_CHANNELS must be on or off, got {other:?}")),
        };
        let mcp = match get("FUWA_MCP").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => true,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_MCP must be on or off, got {other:?}")),
        };
        let profile_effects = match get("FUWA_PROFILE_EFFECTS").as_deref().map(str::trim) {
            None | Some("on" | "true" | "1") => true,
            Some("off" | "false" | "0") => false,
            Some(other) => return Err(format!("FUWA_PROFILE_EFFECTS must be on or off, got {other:?}")),
        };
        let federation = match get("FUWA_FEDERATION").as_deref().map(str::trim) {
            None | Some("off" | "false" | "0") => false,
            Some("on" | "true" | "1") => true,
            Some(other) => return Err(format!("FUWA_FEDERATION must be on or off, got {other:?}")),
        };
        let federation_allow_private = match get("FUWA_FEDERATION_ALLOW_PRIVATE").as_deref().map(str::trim) {
            None | Some("off" | "false" | "0" | "") => false,
            Some("on" | "true" | "1" | "yes") => true,
            Some(other) => return Err(format!("FUWA_FEDERATION_ALLOW_PRIVATE must be on or off, got {other:?}")),
        };
        let call_recordings_keep_days = match get("FUWA_CALL_RECORDINGS_KEEP_DAYS") {
            None => None,
            Some(value) => Some(value.trim().parse::<i64>().ok().filter(|n| *n >= 1).ok_or_else(|| {
                format!("FUWA_CALL_RECORDINGS_KEEP_DAYS must be a whole number of days, 1 or more, got {value:?}")
            })?),
        };
        let list = |key: &str| -> Vec<String> {
            get(key).unwrap_or_default().split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
        };
        let ice_urls = list("FUWA_ICE_URLS");
        crate::settings::check_ice_urls(&ice_urls).map_err(|err| format!("FUWA_ICE_URLS: {err}"))?;
        let media = match (cluster.role, get("FUWA_MEDIA_PORT").as_deref().map(str::trim)) {
            (Role::All | Role::Media, Some("off")) => {
                if cluster.role == Role::Media {
                    return Err("FUWA_MEDIA_PORT can't be off on a media part".into());
                }
                None
            }
            (Role::All | Role::Media, port) => {
                let port = match port {
                    None => crate::rtc::DEFAULT_PORT,
                    Some(port) => port
                        .parse::<u16>()
                        .ok()
                        .ok_or_else(|| format!("FUWA_MEDIA_PORT must be a port number or off, got {port:?}"))?,
                };
                let addresses = list("FUWA_MEDIA_ADDRESSES")
                    .iter()
                    .map(|a| crate::rtc::Advertised::parse(a))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|err| format!("FUWA_MEDIA_ADDRESSES: {err}"))?;
                Some(crate::rtc::MediaConfig { port, addresses })
            }
            _ => None,
        };
        let media_urls = list("FUWA_MEDIA_URL");
        for url in &media_urls {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!("FUWA_MEDIA_URL must be http(s) URLs, got {url:?}"));
            }
            if url.starts_with("http://") && !private_host(url) {
                return Err(format!(
                    "FUWA_MEDIA_URL {url:?} must be https://: plain http:// would send the key in the clear, so it's \
                     only for private names (like http://media:8080 or *.railway.internal) and addresses"
                ));
            }
        }
        let media_urls = media_urls.into_iter().map(|u| u.trim_end_matches('/').to_string()).collect();

        let count = |key: &str, default: Option<usize>| -> Result<Option<usize>, String> {
            match get(key).as_deref().map(str::trim) {
                None => Ok(default),
                Some("unlimited") => Ok(None),
                Some(value) => match value.parse::<usize>() {
                    Ok(n) if n >= 1 => Ok(Some(n)),
                    _ => Err(format!("{key} must be a whole number, 1 or more, or unlimited, got {value:?}")),
                },
            }
        };
        let streams_per_account = count("FUWA_STREAMS_PER_ACCOUNT", Some(crate::streams::PER_ACCOUNT))?;
        let max_streams = count("FUWA_MAX_STREAMS", None)?;
        let write_queue = count("FUWA_WRITE_QUEUE", Some(crate::db::WRITE_QUEUE))?;
        let sign_in_queue = count("FUWA_SIGN_IN_QUEUE", Some(crate::auth::HASH_WAITING))?;

        let replica = match (replica(&get)?, cluster.role) {
            // Only the parts that keep files replicate them: a split
            // instance's directory and shards. One process keeps plain local files.
            (Some(_), Role::All) => {
                return Err("FUWA_S3_BUCKET and FUWA_REPLICA_PATH replicate a split instance's directory and shards \
                            (FUWA_ROLE=directory or shard); an instance run as one process keeps its files on its own disk"
                    .into());
            }
            // Gateways and media parts keep nothing (and may share the others' variables).
            (Some(_), Role::Gateway | Role::Media) => None,
            (replica, _) => replica,
        };

        Ok(Self {
            data_path,
            host: get("FUWA_HOST").unwrap_or_else(|| "0.0.0.0".into()),
            port,
            public_url,
            node_name: get("FUWA_NODE_NAME").unwrap_or_else(|| "Fuwa".into()),
            allowed_origins,
            encryption_key,
            local_accounts,
            linked_accounts,
            linked_issuer,
            sso_accounts,
            server_creation,
            agent_creation,
            admin_token,
            limits,
            telemetry: Telemetry {
                enabled: telemetry_enabled,
                reports_url: get("FUWA_REPORTS_URL").unwrap_or_else(|| {
                    let url = get("FUWA_TELEMETRY_URL").unwrap_or_else(|| DEFAULT_TELEMETRY_URL.into());
                    match url.trim_end_matches('/').strip_suffix("/signals") {
                        Some(base) => format!("{base}/reports"),
                        None => DEFAULT_REPORTS_URL.into(),
                    }
                }),
                url: get("FUWA_TELEMETRY_URL").unwrap_or_else(|| DEFAULT_TELEMETRY_URL.into()),
                hosted,
            },
            web,
            cluster,
            calls,
            call_recordings,
            shared_channels,
            mcp,
            profile_effects,
            federation,
            federation_allow_private,
            call_recordings_keep_days,
            ice_urls,
            turn_secret: get("FUWA_TURN_SECRET").map(|s| s.trim().to_string()).unwrap_or_default(),
            automod_providers: automod_providers(&get)?,
            gifs: gifs(&get)?,
            gif_api_url: gif_api_url(&get)?,
            media,
            media_urls,
            replica,
            streams_per_account,
            max_streams,
            write_queue,
            sign_in_queue,
        })
    }
}

fn replica(get: &impl Fn(&str) -> Option<String>) -> Result<Option<ReplicaConfig>, String> {
    let target = match (get("FUWA_S3_BUCKET"), get("FUWA_REPLICA_PATH")) {
        (Some(_), Some(_)) => return Err("set FUWA_S3_BUCKET or FUWA_REPLICA_PATH, not both".into()),
        (Some(bucket), None) => {
            let region = get("FUWA_S3_REGION").map(|r| r.trim().to_string()).unwrap_or_else(|| "auto".into());
            let endpoint = match get("FUWA_S3_ENDPOINT") {
                Some(endpoint) => endpoint.trim().trim_end_matches('/').to_string(),
                None if region != "auto" => format!("https://s3.{region}.amazonaws.com"),
                None => {
                    return Err("FUWA_S3_ENDPOINT is needed with FUWA_S3_BUCKET (or a FUWA_S3_REGION on AWS)".into());
                }
            };
            if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
                return Err(format!("FUWA_S3_ENDPOINT must start with https://, got {endpoint:?}"));
            }
            let secret = |key: &str| {
                get(key).map(|v| v.trim().to_string()).ok_or_else(|| format!("{key} is needed with FUWA_S3_BUCKET"))
            };
            let path_style = match get("FUWA_S3_PATH_STYLE").as_deref().map(str::trim) {
                None | Some("off" | "false" | "0") => false,
                Some("on" | "true" | "1") => true,
                Some(other) => return Err(format!("FUWA_S3_PATH_STYLE must be on or off, got {other:?}")),
            };
            Target::Bucket {
                s3: S3Config {
                    bucket: bucket.trim().to_string(),
                    endpoint,
                    region,
                    access_key_id: secret("FUWA_S3_ACCESS_KEY_ID")?,
                    secret_access_key: secret("FUWA_S3_SECRET_ACCESS_KEY")?,
                    path_style,
                },
                prefix: get("FUWA_S3_PREFIX").unwrap_or_default().trim().to_string(),
            }
        }
        (None, Some(path)) => Target::Dir(PathBuf::from(path.trim())),
        (None, None) => {
            if get("FUWA_RESTORE").is_some_and(|value| value.trim() != "off") {
                return Err("FUWA_RESTORE needs a replica to restore from: FUWA_S3_BUCKET or FUWA_REPLICA_PATH".into());
            }
            return Ok(None);
        }
    };
    let interval = match get("FUWA_REPLICA_INTERVAL") {
        Some(value) => parse_duration(&value).map_err(|err| format!("FUWA_REPLICA_INTERVAL {err}"))?,
        None => std::time::Duration::from_secs(1),
    };
    let restore = match get("FUWA_RESTORE").as_deref().map(str::trim) {
        None | Some("off") => Restore::Off,
        Some("if-empty") => Restore::IfEmpty,
        Some(other) => return Err(format!("FUWA_RESTORE must be off or if-empty, got {other:?}")),
    };
    Ok(Some(ReplicaConfig { target, interval, restore }))
}

/// Parses a duration like `500ms`, `1s` or `2m` (a bare number is seconds).
pub fn parse_duration(value: &str) -> Result<std::time::Duration, String> {
    let value = value.trim();
    let split = value.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.parse().map_err(|_| format!("must be a duration like 500ms, 1s or 2m, got {value:?}"))?;
    let seconds = match unit.trim() {
        "" | "s" => number,
        "ms" => number / 1000.0,
        "m" => number * 60.0,
        _ => return Err(format!("has an unknown unit in {value:?}; use ms, s or m")),
    };
    if !(0.05..=3600.0).contains(&seconds) {
        return Err(format!("must be between 50ms and 60m, got {value:?}"));
    }
    Ok(std::time::Duration::from_secs_f64(seconds))
}

/// Parses a size like `500MB`, `2GiB` or `1048576`.
pub fn parse_bytes(value: &str) -> Result<i64, String> {
    let value = value.trim();
    let split = value.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.parse().map_err(|_| format!("must be a size like 500MB or 2GiB, got {value:?}"))?;
    let multiplier: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("has an unknown unit in {value:?}; use B, KB, MB, GB, TB, KiB, MiB, GiB or TiB")),
    };
    Ok((number * multiplier).round() as i64)
}

/// The moderation providers the environment turns on (FUWA_JEV_API_KEY,
/// FUWA_CLEF_API_TOKEN with FUWA_CLEF_ACCOUNT_ID), checked like a change
/// from the app.
fn automod_providers(get: &impl Fn(&str) -> Option<String>) -> Result<Vec<crate::automod::providers::Setup>, String> {
    use crate::automod::providers::Setup;
    let mut setups = Vec::new();
    let value = |name: &str| get(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    if let Some(key) = value("FUWA_JEV_API_KEY") {
        let given = pb::AutoModProviderSettings {
            id: "typesafe-jev".into(),
            enabled: true,
            api_key: key,
            ..Default::default()
        };
        setups.push(Setup::from_pb(&given, None).map_err(|err| format!("FUWA_JEV_API_KEY: {err}"))?);
    }
    match (value("FUWA_CLEF_API_TOKEN"), value("FUWA_CLEF_ACCOUNT_ID")) {
        (Some(key), Some(account_id)) => {
            let given = pb::AutoModProviderSettings {
                id: "cloudflare-clef".into(),
                enabled: true,
                api_key: key,
                account_id,
                ..Default::default()
            };
            setups.push(
                Setup::from_pb(&given, None)
                    .map_err(|err| format!("FUWA_CLEF_API_TOKEN or FUWA_CLEF_ACCOUNT_ID: {err}"))?,
            );
        }
        (None, None) => {}
        _ => return Err("FUWA_CLEF_API_TOKEN and FUWA_CLEF_ACCOUNT_ID go together".into()),
    }
    Ok(setups)
}

/// GIF search from FUWA_GIF_PROVIDER and FUWA_GIF_API_KEY, checked like a
/// change from the app.
/// FUWA_GIF_API_URL: a stand-in GIF provider for tests, only ever on this
/// machine, since calls to it skip the public-address check.
fn gif_api_url(get: &impl Fn(&str) -> Option<String>) -> Result<Option<String>, String> {
    let Some(url) =
        get("FUWA_GIF_API_URL").map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    let local = reqwest::Url::parse(&url).ok().filter(|u| {
        let loopback = match u.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        loopback && matches!(u.scheme(), "http" | "https") && u.path() == "/" && u.query().is_none()
    });
    match local {
        Some(_) => Ok(Some(url)),
        None => Err(format!(
            "FUWA_GIF_API_URL is for tests and must be a loopback address like http://127.0.0.1:9000, got {url:?}"
        )),
    }
}

fn gifs(get: &impl Fn(&str) -> Option<String>) -> Result<crate::gifs::Setup, String> {
    let value = |name: &str| get(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let provider = match value("FUWA_GIF_PROVIDER").as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("off") => pb::GifProvider::Unspecified,
        Some("giphy") => pb::GifProvider::Giphy,
        Some("klipy") => pb::GifProvider::Klipy,
        Some(other) => return Err(format!("FUWA_GIF_PROVIDER must be giphy, klipy or off, got {other:?}")),
    };
    let given = pb::GifSettings {
        provider: provider as i32,
        api_key: value("FUWA_GIF_API_KEY").unwrap_or_default(),
        ..Default::default()
    };
    crate::gifs::Setup::from_pb(&given, &crate::gifs::Setup::default())
        .map_err(|err| format!("FUWA_GIF_PROVIDER or FUWA_GIF_API_KEY: {err}"))
}

/// Whether an `http://` URL's host is on a private network: a name with no
/// dots (Docker Compose), localhost, `*.internal` (Railway's private network)
/// or a loopback, private or link-local address. Anything else is reached over
/// the internet and must use https.
fn private_host(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    match parsed.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            !name.contains('.') || name == "localhost" || name.ends_with(".localhost") || name.ends_with(".internal")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_lookup(|key| vars.get(key).cloned())
    }

    #[test]
    fn moderation_providers_can_start_on() {
        assert!(config(&[]).unwrap().automod_providers.is_empty());
        let on = config(&[
            ("FUWA_JEV_API_KEY", "ts-key"),
            ("FUWA_CLEF_API_TOKEN", "cf-token"),
            ("FUWA_CLEF_ACCOUNT_ID", "0123456789abcdef0123456789abcdef"),
        ])
        .unwrap();
        let ids: Vec<&str> = on.automod_providers.iter().filter(|s| s.usable()).map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["typesafe-jev", "cloudflare-clef"]);
        assert!(!format!("{on:?}").contains("cf-token"));
        assert!(config(&[("FUWA_CLEF_API_TOKEN", "cf-token")]).is_err());
        assert!(config(&[("FUWA_CLEF_API_TOKEN", "t"), ("FUWA_CLEF_ACCOUNT_ID", "nope")]).is_err());
    }

    #[test]
    fn defaults_are_open_and_unlimited() {
        let config = config(&[("FUWA_DATA_PATH", "/data")]).unwrap();
        assert_eq!(config.port, 8080);
        assert_eq!(config.public_url, "http://localhost:8080");
        assert_eq!(config.local_accounts, Accounts::Open);
        assert_eq!(config.linked_accounts, Accounts::Open);
        assert_eq!(config.sso_accounts, Accounts::Off);
        assert_eq!(config.linked_issuer, "https://api.waifu.dev");
        assert_eq!(config.server_creation, pb::ServerCreation::Everyone);
        assert_eq!(config.agent_creation, pb::AgentCreation::Everyone);
        assert!(!config.limits.any());
        assert_eq!(config.limits.picture_upload_bytes, None);
        assert_eq!(config.limits.picture_upload_bytes_per_day, None);
        assert!(config.telemetry.enabled);
        assert!(config.encryption_key.is_none());
    }

    #[test]
    fn port_falls_back_to_port() {
        assert_eq!(config(&[("PORT", "4000")]).unwrap().port, 4000);
        assert_eq!(config(&[("PORT", "4000"), ("FUWA_PORT", "5000")]).unwrap().port, 5000);
        assert!(config(&[("FUWA_PORT", "nope")]).is_err());
    }

    #[test]
    fn telemetry_can_be_turned_off() {
        assert!(!config(&[("FUWA_TELEMETRY", "off")]).unwrap().telemetry.enabled);
        assert_eq!(config(&[]).unwrap().telemetry.reports_url, DEFAULT_REPORTS_URL);
        let moved = config(&[("FUWA_TELEMETRY_URL", "http://collector.test/v1/fuwa/signals")]).unwrap();
        assert_eq!(moved.telemetry.reports_url, "http://collector.test/v1/fuwa/reports");
        assert!(!config(&[("DO_NOT_TRACK", "1")]).unwrap().telemetry.enabled);
    }

    #[test]
    fn limits_parse() {
        let config = config(&[
            ("FUWA_LIMIT_MEMBERS", "100"),
            ("FUWA_LIMIT_STORAGE", "500MB"),
            ("FUWA_LIMIT_ATTACHMENT_STORAGE", "2GiB"),
            ("FUWA_LIMIT_PICTURE_UPLOAD", "2MiB"),
            ("FUWA_LIMIT_PICTURE_UPLOADS_PER_DAY", "unlimited"),
            ("FUWA_LIMIT_RECORDING_STORAGE", "20GB"),
            ("FUWA_CALL_RECORDINGS_KEEP_DAYS", "30"),
        ])
        .unwrap();
        assert_eq!(config.limits.members, Some(100));
        assert_eq!(config.limits.storage_bytes, Some(500_000_000));
        assert_eq!(config.limits.attachment_bytes, Some(2 * 1024 * 1024 * 1024));
        assert_eq!(config.limits.picture_upload_bytes, Some(2 * 1024 * 1024));
        assert_eq!(config.limits.picture_upload_bytes_per_day, None);
        assert_eq!(config.limits.recording_bytes, Some(20_000_000_000));
        assert_eq!(config.call_recordings_keep_days, Some(30));
        assert!(config.limits.any());
    }

    #[test]
    fn bad_values_are_explained() {
        assert!(config(&[("FUWA_LOCAL_ACCOUNTS", "maybe")]).unwrap_err().contains("open, closed or off"));
        assert!(config(&[("FUWA_LINKED_ACCOUNTS", "maybe")]).unwrap_err().contains("FUWA_LINKED_ACCOUNTS"));
        assert!(config(&[("FUWA_LINKED_ISSUER", "api.waifu.dev")]).unwrap_err().contains("https://"));
        assert!(config(&[("FUWA_ENCRYPTION_KEY", "short")]).unwrap_err().contains("64 hex"));
        assert!(config(&[("FUWA_LIMIT_STORAGE", "5 parsecs")]).unwrap_err().contains("unknown unit"));
        assert!(config(&[("FUWA_CALL_RECORDINGS_KEEP_DAYS", "0")]).unwrap_err().contains("1 or more"));
    }

    #[test]
    fn media_parts_on_the_internet_need_https() {
        let split = [("FUWA_ROLE", "shard"), ("FUWA_CLUSTER_KEY", "cluster-key-0123456789abcdef0123456789abcdef")];
        let split =
            [&split[..], &[("FUWA_DIRECTORY_URL", "http://directory:8080"), ("FUWA_INTERNAL_URL", "http://s:8080")]]
                .concat();
        let media = |url: &str| config(&[&split[..], &[("FUWA_MEDIA_URL", url)]].concat()).map(|c| c.media_urls);
        for private in [
            "http://media:8080",
            "http://fuwa-media.railway.internal:8080",
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://10.0.0.5:8080",
            "http://192.168.1.2:8080",
            "http://[::1]:8080",
            "http://[fd00::5]:8080",
            "https://media.example.com:8443",
        ] {
            assert!(media(private).is_ok(), "{private}");
        }
        for public in ["http://media.example.com:8443", "http://203.0.113.5:8080", "http://[2001:db8::1]:8080"] {
            assert!(media(public).unwrap_err().contains("must be https://"), "{public}");
        }
    }

    #[test]
    fn the_test_gif_provider_is_only_on_this_machine() {
        let local = config(&[("FUWA_GIF_API_URL", "http://127.0.0.1:9911/")]).unwrap();
        assert_eq!(local.gif_api_url.as_deref(), Some("http://127.0.0.1:9911"));
        assert!(config(&[("FUWA_GIF_API_URL", "http://localhost:9911")]).is_ok());
        for elsewhere in ["http://10.0.0.5", "https://api.giphy.com", "http://127.0.0.1.example.com", "file:///etc"] {
            assert!(config(&[("FUWA_GIF_API_URL", elsewhere)]).is_err(), "{elsewhere}");
        }
    }

    #[test]
    fn the_replica_is_off_until_a_bucket_or_path_is_set() {
        assert!(config(&[]).unwrap().replica.is_none());
        assert!(config(&[("FUWA_RESTORE", "if-empty")]).unwrap_err().contains("needs a replica"));
        let split = [("FUWA_ROLE", "directory"), ("FUWA_CLUSTER_KEY", "cluster-key-0123456789abcdef0123456789abcdef")];
        // The test's own variables win (the last one set is kept).
        let config = |vars: &[(&str, &str)]| config(&[&split[..], vars].concat());

        let railway = config(&[
            ("FUWA_S3_BUCKET", "fuwa-replica-abc123"),
            ("FUWA_S3_ENDPOINT", "https://t3.storageapi.dev/"),
            ("FUWA_S3_REGION", "auto"),
            ("FUWA_S3_ACCESS_KEY_ID", "id"),
            ("FUWA_S3_SECRET_ACCESS_KEY", "secret"),
            ("FUWA_RESTORE", "if-empty"),
        ])
        .unwrap()
        .replica
        .unwrap();
        assert_eq!(railway.interval, std::time::Duration::from_secs(1));
        assert_eq!(railway.restore, Restore::IfEmpty);
        let Target::Bucket { s3, prefix } = &railway.target else { panic!("not a bucket") };
        assert_eq!((s3.endpoint.as_str(), s3.path_style, prefix.as_str()), ("https://t3.storageapi.dev", false, ""));
        assert!(!format!("{railway:?}").contains("secret"));
        let keyed = config(&[("FUWA_ADMIN_TOKEN", "admin-token-0123456789abcdef0123456789")]).unwrap();
        let printed = format!("{keyed:?}");
        assert!(!printed.contains("admin-token-0123") && !printed.contains("cluster-key-0123"), "{printed}");
        assert!(printed.contains("admin_token: Some(***)") && printed.contains("key: Some(***)"));

        let aws = config(&[
            ("FUWA_S3_BUCKET", "b"),
            ("FUWA_S3_REGION", "eu-west-1"),
            ("FUWA_S3_ACCESS_KEY_ID", "id"),
            ("FUWA_S3_SECRET_ACCESS_KEY", "s"),
        ]);
        let Target::Bucket { s3, .. } = aws.unwrap().replica.unwrap().target else { panic!("not a bucket") };
        assert_eq!(s3.endpoint, "https://s3.eu-west-1.amazonaws.com");
        assert!(
            config(&[("FUWA_S3_BUCKET", "b"), ("FUWA_S3_ENDPOINT", "https://x")])
                .unwrap_err()
                .contains("ACCESS_KEY_ID")
        );
        assert!(config(&[("FUWA_S3_BUCKET", "b")]).unwrap_err().contains("FUWA_S3_ENDPOINT"));

        let dir =
            config(&[("FUWA_REPLICA_PATH", "/backups"), ("FUWA_REPLICA_INTERVAL", "250ms")]).unwrap().replica.unwrap();
        assert!(matches!(dir.target, Target::Dir(ref path) if path == std::path::Path::new("/backups")));

        // Only for the parts of a split instance that keep files.
        let one_process = super::tests::config(&[("FUWA_REPLICA_PATH", "/backups")]).unwrap_err();
        assert!(one_process.contains("FUWA_ROLE=directory or shard"), "{one_process}");
        let gateway = config(&[
            ("FUWA_ROLE", "gateway"),
            ("FUWA_DIRECTORY_URL", "http://directory:8080"),
            ("FUWA_REPLICA_PATH", "/backups"),
        ]);
        assert!(gateway.unwrap().replica.is_none());
        assert_eq!(dir.interval, std::time::Duration::from_millis(250));
        assert!(config(&[("FUWA_REPLICA_PATH", "/b"), ("FUWA_REPLICA_INTERVAL", "1 fortnight")]).is_err());
        assert!(config(&[("FUWA_REPLICA_PATH", "/b"), ("FUWA_S3_BUCKET", "b")]).unwrap_err().contains("not both"));
    }
}
