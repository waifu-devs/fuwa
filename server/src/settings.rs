//! Instance settings that admins change from a client while the instance runs.
//! Each one starts from its `FUWA_*` environment variable (or built-in default);
//! a value set through the API is stored in node.db and wins over it.

use serde_json::Value;

use crate::config::{Accounts, Config, Limits};
use crate::error::{Error, Result};
use crate::pb;

/// Every setting, as the field path the API and node.db use for it.
pub const FIELDS: &[&str] = &[
    "name",
    "public_url",
    "allowed_origins",
    "local_accounts",
    "linked_accounts",
    "linked_issuer",
    "server_creation",
    "servers_per_account",
    "default_limits.members",
    "default_limits.channels",
    "default_limits.storage_bytes",
    "default_limits.attachment_bytes",
    "picture_upload_bytes",
    "telemetry",
    "web",
];

/// The settings in force.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub name: String,
    pub public_url: String,
    /// `["*"]` for any origin.
    pub allowed_origins: Vec<String>,
    pub local_accounts: Accounts,
    pub linked_accounts: Accounts,
    pub linked_issuer: String,
    pub server_creation: pb::ServerCreation,
    pub limits: Limits,
    pub telemetry: bool,
    pub web: bool,
}

impl Settings {
    /// The settings with nothing changed from a client: the environment's values.
    pub fn defaults(config: &Config) -> Self {
        Self {
            name: config.node_name.clone(),
            public_url: config.public_url.clone(),
            allowed_origins: config.allowed_origins.clone(),
            local_accounts: config.local_accounts,
            linked_accounts: config.linked_accounts,
            linked_issuer: config.linked_issuer.clone(),
            server_creation: config.server_creation,
            limits: config.limits.clone(),
            telemetry: config.telemetry.enabled,
            web: config.web,
        }
    }

    /// The defaults with stored changes applied. A stored value that no longer
    /// reads (say, from a newer version) is skipped with a warning.
    pub fn load(config: &Config, stored: &[(String, String)]) -> Self {
        let mut settings = Self::defaults(config);
        for (field, json) in stored {
            let applied = serde_json::from_str::<Value>(json)
                .map_err(|err| Error::invalid(err.to_string()))
                .and_then(|value| settings.set_json(field, &value));
            if let Err(err) = applied {
                tracing::warn!(setting = %field, error = %err, "ignoring a stored setting");
            }
        }
        settings
    }

    /// The settings another part of a split instance sent: every field taken
    /// from it, falling back to this process's own default where one doesn't read.
    pub fn from_pb(config: &Config, from: &pb::InstanceSettings) -> Self {
        let mut settings = Self::defaults(config);
        for field in FIELDS {
            if let Err(err) = settings.set_from_pb(field, from) {
                tracing::warn!(setting = %field, error = %err, "ignoring a setting the directory sent");
            }
        }
        settings
    }

    /// Whether a browser on `origin` may call the API.
    pub fn allows_origin(&self, origin: &[u8]) -> bool {
        self.allowed_origins.iter().any(|allowed| allowed == "*" || allowed.as_bytes() == origin)
    }

    /// Whether linked accounts can sign in: they're on, and the issuer can send
    /// people back to the public URL (https, or this machine while testing).
    pub fn linked_sign_in(&self) -> bool {
        self.linked_accounts.sign_in() && crate::linked::can_return_to(&self.public_url)
    }

    /// Whether someone new can sign in with waifu.dev and get an account.
    pub fn linked_sign_up(&self) -> bool {
        self.linked_accounts.sign_up() && self.linked_sign_in()
    }

    pub fn to_pb(&self) -> pb::InstanceSettings {
        let limits = &self.limits;
        pb::InstanceSettings {
            name: self.name.clone(),
            public_url: self.public_url.clone(),
            allowed_origins: self.allowed_origins.clone(),
            local_accounts: match self.local_accounts {
                Accounts::Open => pb::LocalAccounts::Open,
                Accounts::Closed => pb::LocalAccounts::Closed,
                Accounts::Off => pb::LocalAccounts::Off,
            } as i32,
            linked_accounts: match self.linked_accounts {
                Accounts::Open => pb::LinkedAccounts::Open,
                Accounts::Closed => pb::LinkedAccounts::Closed,
                Accounts::Off => pb::LinkedAccounts::Off,
            } as i32,
            linked_issuer: self.linked_issuer.clone(),
            server_creation: self.server_creation as i32,
            servers_per_account: limits.servers_per_account,
            default_limits: Some(pb::ServerLimits {
                members: limits.members,
                channels: limits.channels,
                storage_bytes: limits.storage_bytes,
                attachment_bytes: limits.attachment_bytes,
            }),
            telemetry: self.telemetry,
            web: self.web,
            picture_upload_bytes: limits.picture_upload_bytes,
        }
    }

    /// Sets one field from a request, checking it.
    pub fn set_from_pb(&mut self, field: &str, from: &pb::InstanceSettings) -> Result<()> {
        let limits = from.default_limits.unwrap_or_default();
        let value = match field {
            "name" => Value::from(from.name.clone()),
            "public_url" => Value::from(from.public_url.clone()),
            "allowed_origins" => Value::from(from.allowed_origins.clone()),
            "local_accounts" => Value::from(
                match pb::LocalAccounts::try_from(from.local_accounts).unwrap_or(pb::LocalAccounts::Unspecified) {
                    pb::LocalAccounts::Open => "open",
                    pb::LocalAccounts::Closed => "closed",
                    pb::LocalAccounts::Off => "off",
                    pb::LocalAccounts::Unspecified => "",
                },
            ),
            "linked_accounts" => Value::from(
                match pb::LinkedAccounts::try_from(from.linked_accounts).unwrap_or(pb::LinkedAccounts::Unspecified) {
                    pb::LinkedAccounts::Open => "open",
                    pb::LinkedAccounts::Closed => "closed",
                    pb::LinkedAccounts::Off => "off",
                    pb::LinkedAccounts::Unspecified => "",
                },
            ),
            "linked_issuer" => Value::from(from.linked_issuer.clone()),
            "server_creation" => Value::from(
                match pb::ServerCreation::try_from(from.server_creation).unwrap_or(pb::ServerCreation::Unspecified) {
                    pb::ServerCreation::Everyone => "everyone",
                    pb::ServerCreation::Admins => "admins",
                    pb::ServerCreation::Disabled => "off",
                    pb::ServerCreation::Unspecified => "",
                },
            ),
            "servers_per_account" => Value::from(from.servers_per_account),
            "default_limits.members" => Value::from(limits.members),
            "default_limits.channels" => Value::from(limits.channels),
            "default_limits.storage_bytes" => Value::from(limits.storage_bytes),
            "default_limits.attachment_bytes" => Value::from(limits.attachment_bytes),
            "picture_upload_bytes" => Value::from(from.picture_upload_bytes),
            "telemetry" => Value::from(from.telemetry),
            "web" => Value::from(from.web),
            other => return Err(unknown(other)),
        };
        self.set_json(field, &value)
    }

    /// One field as it's stored.
    pub fn get_json(&self, field: &str) -> Result<Value> {
        let limits = &self.limits;
        Ok(match field {
            "name" => Value::from(self.name.clone()),
            "public_url" => Value::from(self.public_url.clone()),
            "allowed_origins" => Value::from(self.allowed_origins.clone()),
            "local_accounts" => Value::from(self.local_accounts.as_str()),
            "linked_accounts" => Value::from(self.linked_accounts.as_str()),
            "linked_issuer" => Value::from(self.linked_issuer.clone()),
            "server_creation" => Value::from(match self.server_creation {
                pb::ServerCreation::Admins => "admins",
                pb::ServerCreation::Disabled => "off",
                _ => "everyone",
            }),
            "servers_per_account" => Value::from(limits.servers_per_account),
            "default_limits.members" => Value::from(limits.members),
            "default_limits.channels" => Value::from(limits.channels),
            "default_limits.storage_bytes" => Value::from(limits.storage_bytes),
            "default_limits.attachment_bytes" => Value::from(limits.attachment_bytes),
            "picture_upload_bytes" => Value::from(limits.picture_upload_bytes),
            "telemetry" => Value::from(self.telemetry),
            "web" => Value::from(self.web),
            other => return Err(unknown(other)),
        })
    }

    /// Sets one field from its stored form, checking it.
    fn set_json(&mut self, field: &str, value: &Value) -> Result<()> {
        match field {
            "name" => self.name = name(value)?,
            "public_url" => self.public_url = public_url(value)?,
            "allowed_origins" => self.allowed_origins = origins(value)?,
            "local_accounts" | "linked_accounts" => {
                let accounts = value
                    .as_str()
                    .and_then(Accounts::parse)
                    .ok_or_else(|| Error::invalid(format!("{field} must be open, closed or off")))?;
                if field == "local_accounts" {
                    self.local_accounts = accounts;
                } else {
                    self.linked_accounts = accounts;
                }
            }
            "linked_issuer" => self.linked_issuer = issuer(value)?,
            "server_creation" => {
                self.server_creation = match value.as_str() {
                    Some("everyone") => pb::ServerCreation::Everyone,
                    Some("admins") => pb::ServerCreation::Admins,
                    Some("off") => pb::ServerCreation::Disabled,
                    _ => return Err(Error::invalid("server_creation must be everyone, admins or off")),
                }
            }
            "servers_per_account" => self.limits.servers_per_account = cap(field, value)?,
            "default_limits.members" => self.limits.members = cap(field, value)?,
            "default_limits.channels" => self.limits.channels = cap(field, value)?,
            "default_limits.storage_bytes" => self.limits.storage_bytes = cap(field, value)?,
            "default_limits.attachment_bytes" => self.limits.attachment_bytes = cap(field, value)?,
            "picture_upload_bytes" => self.limits.picture_upload_bytes = cap(field, value)?,
            "telemetry" => self.telemetry = flag(field, value)?,
            "web" => self.web = flag(field, value)?,
            other => return Err(unknown(other)),
        }
        Ok(())
    }
}

/// Turns request field paths into setting names: known ones pass, and
/// "default_limits" stands for all four limits.
pub fn expand(paths: &[String]) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    for path in paths {
        let path = path.trim();
        if path == "default_limits" {
            fields.extend(FIELDS.iter().filter(|f| f.starts_with("default_limits.")).map(|f| f.to_string()));
        } else if FIELDS.contains(&path) {
            fields.push(path.to_string());
        } else {
            return Err(unknown(path));
        }
    }
    fields.sort();
    fields.dedup();
    Ok(fields)
}

fn unknown(field: &str) -> Error {
    Error::invalid(format!("{field:?} isn't a setting; settings are {}", FIELDS.join(", ")))
}

fn name(value: &Value) -> Result<String> {
    let name = value.as_str().unwrap_or_default().trim();
    let length = name.chars().count();
    if !(1..=64).contains(&length) {
        return Err(Error::invalid("name must be 1 to 64 characters"));
    }
    Ok(name.to_string())
}

fn public_url(value: &Value) -> Result<String> {
    let url = value.as_str().unwrap_or_default().trim().trim_end_matches('/');
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    if url.len() > 2048 || rest.is_none_or(|rest| rest.is_empty() || rest.contains(char::is_whitespace)) {
        return Err(Error::invalid("public_url must be an http(s) URL, like https://chat.example.com"));
    }
    Ok(url.to_string())
}

fn issuer(value: &Value) -> Result<String> {
    let url = value.as_str().unwrap_or_default().trim().trim_end_matches('/');
    if url.len() > 2048 || !crate::linked::can_return_to(url) || url.contains(['?', '#', ' ']) {
        return Err(Error::invalid("linked_issuer must be an https URL, like https://api.waifu.dev"));
    }
    Ok(url.to_string())
}

fn origins(value: &Value) -> Result<Vec<String>> {
    let invalid =
        || Error::invalid("allowed_origins must be \"*\" or up to 50 origins like https://app.example.com (no path)");
    let list = value.as_array().ok_or_else(invalid)?;
    let mut origins = Vec::new();
    for origin in list {
        let origin = origin.as_str().ok_or_else(invalid)?.trim().trim_end_matches('/');
        if origin.is_empty() {
            continue;
        }
        if origin == "*" {
            return Ok(vec!["*".into()]);
        }
        let host = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://")).ok_or_else(invalid)?;
        if host.is_empty() || host.contains(['/', '?', '#', ' ']) || origin.len() > 255 {
            return Err(invalid());
        }
        let origin = origin.to_ascii_lowercase();
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    if origins.is_empty() || origins.len() > 50 {
        return Err(invalid());
    }
    Ok(origins)
}

fn cap(field: &str, value: &Value) -> Result<Option<i64>> {
    match value {
        Value::Null => Ok(None),
        _ => match value.as_i64() {
            Some(n) if n >= 0 => Ok(Some(n)),
            _ => Err(Error::invalid(format!("{field} must be a whole number of 0 or more, or unset for no cap"))),
        },
    }
}

fn flag(field: &str, value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(|| Error::invalid(format!("{field} must be true or false")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::from_lookup(|key| (key == "FUWA_NODE_NAME").then(|| "Env name".to_string())).unwrap()
    }

    #[test]
    fn stored_values_win_over_the_environment() {
        let stored = vec![
            ("name".to_string(), "\"Set here\"".to_string()),
            ("default_limits.members".to_string(), "50".to_string()),
            ("local_accounts".to_string(), "\"closed\"".to_string()),
            ("telemetry".to_string(), "false".to_string()),
        ];
        let settings = Settings::load(&config(), &stored);
        assert_eq!(settings.name, "Set here");
        assert_eq!(settings.limits.members, Some(50));
        assert_eq!(settings.local_accounts, Accounts::Closed);
        assert!(!settings.telemetry);
        assert_eq!(Settings::defaults(&config()).name, "Env name");
    }

    #[test]
    fn settings_survive_the_trip_to_another_part() {
        let stored = vec![
            ("name".to_string(), "\"Set here\"".to_string()),
            ("allowed_origins".to_string(), "[\"https://app.example.com\"]".to_string()),
            ("server_creation".to_string(), "\"admins\"".to_string()),
            ("default_limits.storage_bytes".to_string(), "5000".to_string()),
            ("picture_upload_bytes".to_string(), "1000".to_string()),
            ("web".to_string(), "false".to_string()),
            ("linked_accounts".to_string(), "\"closed\"".to_string()),
            ("linked_issuer".to_string(), "\"https://id.example.com\"".to_string()),
        ];
        let settings = Settings::load(&config(), &stored);
        assert_eq!(settings.linked_accounts, Accounts::Closed);
        assert_eq!(Settings::from_pb(&config(), &settings.to_pb()), settings);
    }

    #[test]
    fn unreadable_stored_values_fall_back_to_defaults() {
        let stored = vec![
            ("name".to_string(), "not json".to_string()),
            ("local_accounts".to_string(), "\"sometimes\"".to_string()),
            ("from_the_future".to_string(), "1".to_string()),
        ];
        assert_eq!(Settings::load(&config(), &stored), Settings::defaults(&config()));
    }

    #[test]
    fn every_field_round_trips() {
        let mut settings = Settings::defaults(&config());
        settings.limits.channels = Some(7);
        settings.allowed_origins = vec!["https://a.example".into(), "http://localhost:5173".into()];
        for field in FIELDS {
            let mut copy = Settings::defaults(&config());
            copy.set_json(field, &settings.get_json(field).unwrap()).unwrap();
            assert_eq!(copy.get_json(field).unwrap(), settings.get_json(field).unwrap(), "{field}");
            let mut from_pb = Settings::defaults(&config());
            from_pb.set_from_pb(field, &settings.to_pb()).unwrap();
            assert_eq!(from_pb.get_json(field).unwrap(), settings.get_json(field).unwrap(), "{field} via pb");
        }
    }

    #[test]
    fn bad_values_are_refused() {
        let mut s = Settings::defaults(&config());
        assert!(s.set_json("name", &Value::from("  ")).is_err());
        assert!(s.set_json("public_url", &Value::from("chat.example.com")).is_err());
        assert!(s.set_json("allowed_origins", &serde_json::json!(["https://a.example/path"])).is_err());
        assert!(s.set_json("allowed_origins", &serde_json::json!([])).is_err());
        assert!(s.set_json("default_limits.members", &Value::from(-1)).is_err());
        assert!(s.set_json("telemetry", &Value::from("yes")).is_err());
        assert!(s.set_json("linked_accounts", &Value::from("sometimes")).is_err());
        assert!(s.set_json("linked_issuer", &Value::from("http://id.example.com")).is_err());
        assert!(s.set_json("linked_issuer", &Value::from("https://id.example.com/?x")).is_err());
        assert!(s.set_json("linked_issuer", &Value::from("http://localhost:4000/")).is_ok());
        assert_eq!(s.linked_issuer, "http://localhost:4000");
        assert!(expand(&["nope".into()]).is_err());
        assert_eq!(expand(&["default_limits".into()]).unwrap().len(), 4);
    }

    #[test]
    fn linked_sign_in_needs_an_address_to_come_back_to() {
        let mut s = Settings::defaults(&config());
        assert!(s.linked_sign_in() && s.linked_sign_up(), "localhost works while testing");
        s.set_json("public_url", &Value::from("http://192.168.1.5:8080")).unwrap();
        assert!(!s.linked_sign_in() && !s.linked_sign_up());
        s.set_json("public_url", &Value::from("https://chat.example.com")).unwrap();
        s.set_json("linked_accounts", &Value::from("closed")).unwrap();
        assert!(s.linked_sign_in() && !s.linked_sign_up());
        s.set_json("linked_accounts", &Value::from("off")).unwrap();
        assert!(!s.linked_sign_in());
    }

    #[test]
    fn origins_match_exactly_or_any() {
        let mut s = Settings::defaults(&config());
        assert!(s.allows_origin(b"https://anything.example"));
        s.set_json("allowed_origins", &serde_json::json!(["https://App.example/"])).unwrap();
        assert_eq!(s.allowed_origins, ["https://app.example"]);
        assert!(s.allows_origin(b"https://app.example"));
        assert!(!s.allows_origin(b"https://evil.example"));
    }
}
