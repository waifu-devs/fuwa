//! Instance configuration, from `FUWA_*` environment variables. A `.env` file in
//! the working directory is read first; real environment variables win over it.

use std::env;
use std::path::PathBuf;

use crate::db::EncryptionKey;
use crate::pb;

#[derive(Debug, Clone)]
pub struct Config {
    /// Where node.db and the servers/ directory live. FUWA_DATA_PATH, default ~/.fuwa.
    pub data_path: PathBuf,
    /// FUWA_HOST, default 0.0.0.0.
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
    pub local_accounts: LocalAccounts,
    /// FUWA_SERVER_CREATION: everyone (default) | admins | off.
    pub server_creation: pb::ServerCreation,
    /// FUWA_ADMIN_TOKEN: a bearer token with instance-admin rights, for a control
    /// plane or scripts. Unset means only admin accounts are admins.
    pub admin_token: Option<String>,
    /// FUWA_LIMIT_*: caps every server gets unless it has its own. Unlimited by default.
    pub limits: Limits,
    pub telemetry: Telemetry,
    /// FUWA_WEB: on (default) | off. Serves the web client on / when the binary
    /// was built with it (the Docker image and release builds are).
    pub web: bool,
}

/// Whether standalone accounts (username and password, kept on this instance) are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAccounts {
    /// Anyone can sign up.
    Open,
    /// Existing accounts can sign in; nobody new can sign up.
    Closed,
    /// No standalone accounts at all.
    Off,
}

impl LocalAccounts {
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
}

/// Instance-wide caps. `None` is unlimited.
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
}

impl Limits {
    pub fn any(&self) -> bool {
        self.servers_per_account.is_some()
            || self.members.is_some()
            || self.channels.is_some()
            || self.storage_bytes.is_some()
            || self.attachment_bytes.is_some()
    }
}

/// The anonymous usage signal. See README.md for exactly what it contains.
#[derive(Debug, Clone)]
pub struct Telemetry {
    /// FUWA_TELEMETRY: on (default) | off. DO_NOT_TRACK=1 also turns it off.
    pub enabled: bool,
    /// FUWA_TELEMETRY_URL, default https://analytics.waifu.dev/v1/fuwa/signals.
    pub url: String,
    /// FUWA_HOSTING: self_hosted (default) | hosted. Only Waifu Devs' own hosted
    /// instance says hosted, so the signal can tell the two apart.
    pub hosted: bool,
}

pub const DEFAULT_TELEMETRY_URL: &str = "https://analytics.waifu.dev/v1/fuwa/signals";

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

        let local_accounts = match get("FUWA_LOCAL_ACCOUNTS").as_deref().map(str::trim) {
            None | Some("open") => LocalAccounts::Open,
            Some("closed") => LocalAccounts::Closed,
            Some("off") => LocalAccounts::Off,
            Some(other) => return Err(format!("FUWA_LOCAL_ACCOUNTS must be open, closed or off, got {other:?}")),
        };

        let server_creation = match get("FUWA_SERVER_CREATION").as_deref().map(str::trim) {
            None | Some("everyone") => pb::ServerCreation::Everyone,
            Some("admins") => pb::ServerCreation::Admins,
            Some("off") => pb::ServerCreation::Disabled,
            Some(other) => {
                return Err(format!("FUWA_SERVER_CREATION must be everyone, admins or off, got {other:?}"));
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
        let limits = Limits {
            servers_per_account: count("FUWA_LIMIT_SERVERS_PER_ACCOUNT")?,
            members: count("FUWA_LIMIT_MEMBERS")?,
            channels: count("FUWA_LIMIT_CHANNELS")?,
            storage_bytes: bytes("FUWA_LIMIT_STORAGE")?,
            attachment_bytes: bytes("FUWA_LIMIT_ATTACHMENT_STORAGE")?,
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

        Ok(Self {
            data_path,
            host: get("FUWA_HOST").unwrap_or_else(|| "0.0.0.0".into()),
            port,
            public_url,
            node_name: get("FUWA_NODE_NAME").unwrap_or_else(|| "Fuwa".into()),
            allowed_origins,
            encryption_key,
            local_accounts,
            server_creation,
            admin_token,
            limits,
            telemetry: Telemetry {
                enabled: telemetry_enabled,
                url: get("FUWA_TELEMETRY_URL").unwrap_or_else(|| DEFAULT_TELEMETRY_URL.into()),
                hosted,
            },
            web,
        })
    }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, String> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Config::from_lookup(|key| vars.get(key).cloned())
    }

    #[test]
    fn defaults_are_open_and_unlimited() {
        let config = config(&[("FUWA_DATA_PATH", "/data")]).unwrap();
        assert_eq!(config.port, 8080);
        assert_eq!(config.public_url, "http://localhost:8080");
        assert_eq!(config.local_accounts, LocalAccounts::Open);
        assert_eq!(config.server_creation, pb::ServerCreation::Everyone);
        assert!(!config.limits.any());
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
        assert!(!config(&[("DO_NOT_TRACK", "1")]).unwrap().telemetry.enabled);
    }

    #[test]
    fn limits_parse() {
        let config = config(&[
            ("FUWA_LIMIT_MEMBERS", "100"),
            ("FUWA_LIMIT_STORAGE", "500MB"),
            ("FUWA_LIMIT_ATTACHMENT_STORAGE", "2GiB"),
        ])
        .unwrap();
        assert_eq!(config.limits.members, Some(100));
        assert_eq!(config.limits.storage_bytes, Some(500_000_000));
        assert_eq!(config.limits.attachment_bytes, Some(2 * 1024 * 1024 * 1024));
        assert!(config.limits.any());
    }

    #[test]
    fn bad_values_are_explained() {
        assert!(config(&[("FUWA_LOCAL_ACCOUNTS", "maybe")]).unwrap_err().contains("open, closed or off"));
        assert!(config(&[("FUWA_ENCRYPTION_KEY", "short")]).unwrap_err().contains("64 hex"));
        assert!(config(&[("FUWA_LIMIT_STORAGE", "5 parsecs")]).unwrap_err().contains("unknown unit"));
    }
}
