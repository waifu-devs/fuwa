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
    "sso_accounts",
    "sso_provider",
    "server_creation",
    "agent_creation",
    "servers_per_account",
    "default_limits.members",
    "default_limits.channels",
    "default_limits.storage_bytes",
    "default_limits.attachment_bytes",
    "default_limits.emojis",
    "default_limits.recording_bytes",
    "picture_upload_bytes",
    "picture_upload_bytes_per_day",
    "attachment_upload_bytes",
    "attachment_upload_bytes_per_day",
    "automod_checks_per_day",
    "voice_message_seconds",
    "voice_message_bytes",
    "voice_message_bytes_per_day",
    "shared_remote_sends_per_minute",
    "shared_remote_people",
    "telemetry",
    "web",
    "calls",
    "call_recordings",
    "shared_channels",
    "mcp",
    "profile_effects",
    "federation",
    "federation_blocked_hosts",
    "call_recordings_keep_days",
    "ice_urls",
    "turn_secret",
    "automod_providers",
    "gifs",
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
    pub sso_accounts: Accounts,
    /// Unset (`Protocol::None`) until an admin sets one up.
    pub sso_provider: crate::sso::Provider,
    pub server_creation: pb::ServerCreation,
    pub agent_creation: pb::AgentCreation,
    pub limits: Limits,
    pub telemetry: bool,
    pub web: bool,
    pub calls: bool,
    pub call_recordings: bool,
    pub shared_channels: bool,
    /// Agents may use the instance through MCP, at /mcp (docs/mcp.md).
    pub mcp: bool,
    /// People may put an effect on their profile card.
    pub profile_effects: bool,
    /// Sharing channels with other fuwa instances (docs/federation.md).
    pub federation: bool,
    /// Instances this one won't talk to, by host name.
    pub federation_blocked_hosts: Vec<String>,
    pub call_recordings_keep_days: Option<i64>,
    pub ice_urls: Vec<String>,
    pub turn_secret: String,
    /// Moderation providers servers' AutoMod can use, keys and all; only
    /// the ones admins have set up.
    pub automod_providers: Vec<crate::automod::providers::Setup>,
    /// GIF search: the provider, its key and caps (`crate::gifs`).
    pub gifs: crate::gifs::Setup,
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
            sso_accounts: config.sso_accounts,
            sso_provider: crate::sso::Provider::default(),
            server_creation: config.server_creation,
            agent_creation: config.agent_creation,
            limits: config.limits.clone(),
            telemetry: config.telemetry.enabled,
            web: config.web,
            calls: config.calls,
            call_recordings: config.call_recordings,
            shared_channels: config.shared_channels,
            mcp: config.mcp,
            profile_effects: config.profile_effects,
            federation: config.federation,
            federation_blocked_hosts: Vec::new(),
            call_recordings_keep_days: config.call_recordings_keep_days,
            ice_urls: config.ice_urls.clone(),
            turn_secret: config.turn_secret.clone(),
            automod_providers: config.automod_providers.clone(),
            gifs: config.gifs.clone(),
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
            // Sent apart, keys and all (`WatchResponse.automod_providers`).
            if *field == "automod_providers" {
                continue;
            }
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

    /// Whether people can sign in through the instance's identity provider.
    pub fn sso_sign_in(&self) -> bool {
        self.sso_accounts.sign_in() && self.sso_provider.ready().is_ok()
    }

    /// Whether someone new can sign in through it and get an account.
    pub fn sso_sign_up(&self) -> bool {
        self.sso_accounts.sign_up() && self.sso_sign_in()
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
            sso_accounts: match self.sso_accounts {
                Accounts::Open => pb::SsoAccounts::Open,
                Accounts::Closed => pb::SsoAccounts::Closed,
                Accounts::Off => pb::SsoAccounts::Off,
            } as i32,
            sso_provider: Some(self.sso_provider.to_pb()),
            server_creation: self.server_creation as i32,
            agent_creation: self.agent_creation as i32,
            servers_per_account: limits.servers_per_account,
            default_limits: Some(pb::ServerLimits {
                members: limits.members,
                channels: limits.channels,
                storage_bytes: limits.storage_bytes,
                attachment_bytes: limits.attachment_bytes,
                emojis: limits.emojis,
                recording_bytes: limits.recording_bytes,
                // Its own field, automod_checks_per_day.
                automod_checks_per_day: None,
            }),
            telemetry: self.telemetry,
            web: self.web,
            picture_upload_bytes: limits.picture_upload_bytes,
            picture_upload_bytes_per_day: limits.picture_upload_bytes_per_day,
            attachment_upload_bytes: limits.attachment_upload_bytes,
            attachment_upload_bytes_per_day: limits.attachment_upload_bytes_per_day,
            automod_checks_per_day: limits.automod_checks_per_day,
            voice_message_seconds: limits.voice_message_seconds,
            voice_message_bytes: limits.voice_message_bytes,
            voice_message_bytes_per_day: limits.voice_message_bytes_per_day,
            shared_remote_sends_per_minute: limits.shared_remote_sends_per_minute,
            shared_remote_people: limits.shared_remote_people,
            calls: self.calls,
            call_recordings: self.call_recordings,
            shared_channels: self.shared_channels,
            mcp: self.mcp,
            profile_effects: self.profile_effects,
            federation: self.federation,
            federation_blocked_hosts: self.federation_blocked_hosts.clone(),
            call_recordings_keep_days: self.call_recordings_keep_days,
            ice_urls: self.ice_urls.clone(),
            turn_secret: self.turn_secret.clone(),
            turn_secret_set: !self.turn_secret.is_empty(),
            turn_secret_hint: hint(&self.turn_secret),
            automod_providers: crate::automod::providers::complete(&self.automod_providers)
                .iter()
                .map(|setup| setup.to_pb(false))
                .collect(),
            // Only the directory asks the provider, and it reads the key from
            // node.db, so the key never goes to gateways, shards or apps.
            gifs: Some(self.gifs.to_pb(false)),
        }
    }

    /// As instance admins see it: secrets stay on the server, shown only as
    /// whether one is saved and its last characters.
    pub fn to_admin_pb(&self) -> pb::InstanceSettings {
        pb::InstanceSettings { turn_secret: String::new(), ..self.to_pb() }
    }

    /// A moderation provider servers can use now, by id.
    pub fn automod_provider(&self, id: &str) -> Option<&crate::automod::providers::Setup> {
        self.automod_providers.iter().find(|setup| setup.id == id && setup.usable())
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
            "sso_accounts" => Value::from(
                match pb::SsoAccounts::try_from(from.sso_accounts).unwrap_or(pb::SsoAccounts::Unspecified) {
                    pb::SsoAccounts::Open => "open",
                    pb::SsoAccounts::Closed => "closed",
                    pb::SsoAccounts::Off => "off",
                    pb::SsoAccounts::Unspecified => "",
                },
            ),
            // The secret is never sent out, so an empty one keeps this one.
            "sso_provider" => {
                let provider =
                    crate::sso::Provider::from_pb(&from.sso_provider.clone().unwrap_or_default(), &self.sso_provider)?;
                serde_json::to_value(provider).map_err(|err| Error::internal(err.to_string()))?
            }
            "server_creation" => Value::from(
                match pb::ServerCreation::try_from(from.server_creation).unwrap_or(pb::ServerCreation::Unspecified) {
                    pb::ServerCreation::Everyone => "everyone",
                    pb::ServerCreation::Admins => "admins",
                    pb::ServerCreation::Disabled => "off",
                    pb::ServerCreation::Unspecified => "",
                },
            ),
            "agent_creation" => Value::from(
                match pb::AgentCreation::try_from(from.agent_creation).unwrap_or(pb::AgentCreation::Unspecified) {
                    pb::AgentCreation::Everyone => "everyone",
                    pb::AgentCreation::Admins => "admins",
                    pb::AgentCreation::Disabled => "off",
                    pb::AgentCreation::Unspecified => "",
                },
            ),
            "servers_per_account" => Value::from(from.servers_per_account),
            "default_limits.members" => Value::from(limits.members),
            "default_limits.channels" => Value::from(limits.channels),
            "default_limits.storage_bytes" => Value::from(limits.storage_bytes),
            "default_limits.attachment_bytes" => Value::from(limits.attachment_bytes),
            "default_limits.emojis" => Value::from(limits.emojis),
            "default_limits.recording_bytes" => Value::from(limits.recording_bytes),
            "picture_upload_bytes" => Value::from(from.picture_upload_bytes),
            "picture_upload_bytes_per_day" => Value::from(from.picture_upload_bytes_per_day),
            "attachment_upload_bytes" => Value::from(from.attachment_upload_bytes),
            "attachment_upload_bytes_per_day" => Value::from(from.attachment_upload_bytes_per_day),
            "automod_checks_per_day" => Value::from(from.automod_checks_per_day),
            "voice_message_seconds" => Value::from(from.voice_message_seconds),
            "voice_message_bytes" => Value::from(from.voice_message_bytes),
            "voice_message_bytes_per_day" => Value::from(from.voice_message_bytes_per_day),
            "shared_remote_sends_per_minute" => Value::from(from.shared_remote_sends_per_minute),
            "shared_remote_people" => Value::from(from.shared_remote_people),
            "telemetry" => Value::from(from.telemetry),
            "web" => Value::from(from.web),
            "calls" => Value::from(from.calls),
            "call_recordings" => Value::from(from.call_recordings),
            "shared_channels" => Value::from(from.shared_channels),
            "mcp" => Value::from(from.mcp),
            "profile_effects" => Value::from(from.profile_effects),
            "federation" => Value::from(from.federation),
            "federation_blocked_hosts" => Value::from(from.federation_blocked_hosts.clone()),
            "call_recordings_keep_days" => Value::from(from.call_recordings_keep_days),
            "ice_urls" => Value::from(from.ice_urls.clone()),
            "turn_secret" => Value::from(from.turn_secret.clone()),
            // Keys are never sent out, so an empty one keeps the saved key.
            "automod_providers" => {
                let mut setups: Vec<crate::automod::providers::Setup> = Vec::new();
                for provider in &from.automod_providers {
                    let previous = self.automod_providers.iter().find(|s| s.id == provider.id.trim());
                    let setup = crate::automod::providers::Setup::from_pb(provider, previous)?;
                    if setups.iter().any(|s| s.id == setup.id) {
                        return Err(Error::invalid("each moderation provider is set up once"));
                    }
                    setups.push(setup);
                }
                if setups.iter().filter(|s| s.is_custom()).count() > crate::automod::providers::MAX_CUSTOM {
                    return Err(Error::invalid(format!(
                        "at most {} moderation providers of your own",
                        crate::automod::providers::MAX_CUSTOM
                    )));
                }
                // Leave out the ones with nothing set up.
                setups.retain(|s| *s != crate::automod::providers::Setup { id: s.id.clone(), ..Default::default() });
                serde_json::to_value(setups).map_err(|err| Error::internal(err.to_string()))?
            }
            // The key is never sent out, so an empty one keeps the saved key.
            "gifs" => {
                let setup = crate::gifs::Setup::from_pb(&from.gifs.clone().unwrap_or_default(), &self.gifs)?;
                serde_json::to_value(setup).map_err(|err| Error::internal(err.to_string()))?
            }
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
            "sso_accounts" => Value::from(self.sso_accounts.as_str()),
            "sso_provider" => {
                serde_json::to_value(&self.sso_provider).map_err(|err| Error::internal(err.to_string()))?
            }
            "server_creation" => Value::from(match self.server_creation {
                pb::ServerCreation::Admins => "admins",
                pb::ServerCreation::Disabled => "off",
                _ => "everyone",
            }),
            "agent_creation" => Value::from(match self.agent_creation {
                pb::AgentCreation::Admins => "admins",
                pb::AgentCreation::Disabled => "off",
                _ => "everyone",
            }),
            "servers_per_account" => Value::from(limits.servers_per_account),
            "default_limits.members" => Value::from(limits.members),
            "default_limits.channels" => Value::from(limits.channels),
            "default_limits.storage_bytes" => Value::from(limits.storage_bytes),
            "default_limits.attachment_bytes" => Value::from(limits.attachment_bytes),
            "default_limits.emojis" => Value::from(limits.emojis),
            "default_limits.recording_bytes" => Value::from(limits.recording_bytes),
            "picture_upload_bytes" => Value::from(limits.picture_upload_bytes),
            "picture_upload_bytes_per_day" => Value::from(limits.picture_upload_bytes_per_day),
            "attachment_upload_bytes" => Value::from(limits.attachment_upload_bytes),
            "attachment_upload_bytes_per_day" => Value::from(limits.attachment_upload_bytes_per_day),
            "automod_checks_per_day" => Value::from(limits.automod_checks_per_day),
            "voice_message_seconds" => Value::from(limits.voice_message_seconds),
            "voice_message_bytes" => Value::from(limits.voice_message_bytes),
            "voice_message_bytes_per_day" => Value::from(limits.voice_message_bytes_per_day),
            "shared_remote_sends_per_minute" => Value::from(limits.shared_remote_sends_per_minute),
            "shared_remote_people" => Value::from(limits.shared_remote_people),
            "telemetry" => Value::from(self.telemetry),
            "web" => Value::from(self.web),
            "calls" => Value::from(self.calls),
            "call_recordings" => Value::from(self.call_recordings),
            "shared_channels" => Value::from(self.shared_channels),
            "mcp" => Value::from(self.mcp),
            "profile_effects" => Value::from(self.profile_effects),
            "federation" => Value::from(self.federation),
            "federation_blocked_hosts" => Value::from(self.federation_blocked_hosts.clone()),
            "call_recordings_keep_days" => Value::from(self.call_recordings_keep_days),
            "ice_urls" => Value::from(self.ice_urls.clone()),
            "turn_secret" => Value::from(self.turn_secret.clone()),
            "automod_providers" => {
                serde_json::to_value(&self.automod_providers).map_err(|err| Error::internal(err.to_string()))?
            }
            "gifs" => serde_json::to_value(&self.gifs).map_err(|err| Error::internal(err.to_string()))?,
            other => return Err(unknown(other)),
        })
    }

    /// Sets one field from its stored form, checking it.
    fn set_json(&mut self, field: &str, value: &Value) -> Result<()> {
        match field {
            "name" => self.name = name(value)?,
            "public_url" => self.public_url = public_url(value)?,
            "allowed_origins" => self.allowed_origins = origins(value)?,
            "local_accounts" | "linked_accounts" | "sso_accounts" => {
                let accounts = value
                    .as_str()
                    .and_then(Accounts::parse)
                    .ok_or_else(|| Error::invalid(format!("{field} must be open, closed or off")))?;
                match field {
                    "local_accounts" => self.local_accounts = accounts,
                    "linked_accounts" => self.linked_accounts = accounts,
                    _ => self.sso_accounts = accounts,
                }
            }
            "linked_issuer" => self.linked_issuer = issuer(value)?,
            "sso_provider" => {
                self.sso_provider = serde_json::from_value(value.clone())
                    .map_err(|err| Error::invalid(format!("sso_provider doesn't read: {err}")))?
            }
            "server_creation" => {
                self.server_creation = match value.as_str() {
                    Some("everyone") => pb::ServerCreation::Everyone,
                    Some("admins") => pb::ServerCreation::Admins,
                    Some("off") => pb::ServerCreation::Disabled,
                    _ => return Err(Error::invalid("server_creation must be everyone, admins or off")),
                }
            }
            "agent_creation" => {
                self.agent_creation = match value.as_str() {
                    Some("everyone") => pb::AgentCreation::Everyone,
                    Some("admins") => pb::AgentCreation::Admins,
                    Some("off") => pb::AgentCreation::Disabled,
                    _ => return Err(Error::invalid("agent_creation must be everyone, admins or off")),
                }
            }
            "servers_per_account" => self.limits.servers_per_account = cap(field, value)?,
            "default_limits.members" => self.limits.members = cap(field, value)?,
            "default_limits.channels" => self.limits.channels = cap(field, value)?,
            "default_limits.storage_bytes" => self.limits.storage_bytes = cap(field, value)?,
            "default_limits.attachment_bytes" => self.limits.attachment_bytes = cap(field, value)?,
            "default_limits.emojis" => self.limits.emojis = cap(field, value)?,
            "default_limits.recording_bytes" => self.limits.recording_bytes = cap(field, value)?,
            "picture_upload_bytes" => self.limits.picture_upload_bytes = cap(field, value)?,
            "picture_upload_bytes_per_day" => self.limits.picture_upload_bytes_per_day = cap(field, value)?,
            "attachment_upload_bytes" => self.limits.attachment_upload_bytes = cap(field, value)?,
            "attachment_upload_bytes_per_day" => self.limits.attachment_upload_bytes_per_day = cap(field, value)?,
            "automod_checks_per_day" => self.limits.automod_checks_per_day = cap(field, value)?,
            "voice_message_seconds" => self.limits.voice_message_seconds = cap(field, value)?,
            "voice_message_bytes" => self.limits.voice_message_bytes = cap(field, value)?,
            "voice_message_bytes_per_day" => self.limits.voice_message_bytes_per_day = cap(field, value)?,
            "shared_remote_sends_per_minute" => self.limits.shared_remote_sends_per_minute = cap(field, value)?,
            "shared_remote_people" => self.limits.shared_remote_people = cap(field, value)?,
            "telemetry" => self.telemetry = flag(field, value)?,
            "web" => self.web = flag(field, value)?,
            "calls" => self.calls = flag(field, value)?,
            "call_recordings" => self.call_recordings = flag(field, value)?,
            "shared_channels" => self.shared_channels = flag(field, value)?,
            "mcp" => self.mcp = flag(field, value)?,
            "profile_effects" => self.profile_effects = flag(field, value)?,
            "federation" => self.federation = flag(field, value)?,
            "federation_blocked_hosts" => self.federation_blocked_hosts = blocked_hosts(value)?,
            "call_recordings_keep_days" => {
                self.call_recordings_keep_days = match cap(field, value)? {
                    Some(0) => {
                        return Err(Error::invalid(
                            "call_recordings_keep_days must be 1 or more, or unset to keep them",
                        ));
                    }
                    days => days,
                }
            }
            "ice_urls" => {
                let invalid = || Error::invalid("ice_urls must be a list of stun:, turn: or turns: URLs");
                let list = value.as_array().ok_or_else(invalid)?;
                let urls = list
                    .iter()
                    .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(invalid))
                    .collect::<Result<Vec<_>>>()?;
                let urls: Vec<String> = urls.into_iter().filter(|u| !u.is_empty()).collect();
                check_ice_urls(&urls).map_err(Error::invalid)?;
                self.ice_urls = urls;
            }
            "turn_secret" => {
                let secret = value.as_str().ok_or_else(|| Error::invalid("turn_secret must be text"))?.trim();
                if secret.len() > 256 {
                    return Err(Error::invalid("turn_secret can be at most 256 characters"));
                }
                self.turn_secret = secret.to_string();
            }
            "automod_providers" => {
                let setups: Vec<crate::automod::providers::Setup> = serde_json::from_value(value.clone())
                    .map_err(|err| Error::invalid(format!("automod_providers doesn't read: {err}")))?;
                if let Some(setup) = setups.iter().find(|s| s.kind().is_none() && !s.is_custom()) {
                    return Err(Error::invalid(format!("fuwa doesn't know a moderation provider called {}", setup.id)));
                }
                self.automod_providers = setups;
            }
            "gifs" => {
                let setup: crate::gifs::Setup = serde_json::from_value(value.clone())
                    .map_err(|err| Error::invalid(format!("gifs doesn't read: {err}")))?;
                setup.check()?;
                self.gifs = setup;
            }
            other => return Err(unknown(other)),
        }
        Ok(())
    }
}

/// Turns request field paths into setting names: known ones pass, and
/// "default_limits" stands for all the server limits.
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

/// STUN and TURN servers, as apps take them: `stun:`, `turn:` or `turns:`
/// URLs, at most 10.
pub fn check_ice_urls(urls: &[String]) -> std::result::Result<(), String> {
    if urls.len() > 10 {
        return Err("at most 10 STUN or TURN servers".into());
    }
    for url in urls {
        let rest =
            url.strip_prefix("stun:").or_else(|| url.strip_prefix("turns:")).or_else(|| url.strip_prefix("turn:"));
        if url.len() > 512 || rest.is_none_or(|rest| rest.is_empty() || rest.contains(char::is_whitespace)) {
            return Err(format!("{url:?} isn't a stun:, turn: or turns: URL, like turn:turn.example.com:3478"));
        }
    }
    Ok(())
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

/// Up to 500 host names, like chat.example.com, each once. A full address
/// (https://chat.example.com/) is taken for its host.
fn blocked_hosts(value: &Value) -> Result<Vec<String>> {
    let invalid = || Error::invalid("federation_blocked_hosts must be up to 500 host names, like chat.example.com");
    let list = value.as_array().ok_or_else(invalid)?;
    let mut hosts: Vec<String> = Vec::new();
    for entry in list {
        let entry = entry.as_str().ok_or_else(invalid)?.trim();
        if entry.is_empty() {
            continue;
        }
        let host = crate::federation::host_of(entry).ok_or_else(invalid)?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    if hosts.len() > 500 {
        return Err(invalid());
    }
    Ok(hosts)
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

/// The last four characters of a secret long enough that they give little
/// away, so an admin can tell which one is saved; empty otherwise.
pub(crate) fn hint(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() >= 12 { chars[chars.len() - 4..].iter().collect() } else { String::new() }
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
            ("agent_creation".to_string(), "\"off\"".to_string()),
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
        settings.gifs = crate::gifs::Setup {
            provider: crate::gifs::Kind::Klipy,
            api_key: "klipy-key-0123".into(),
            rating: "g".into(),
            gif_bytes: Some(5_000_000),
            searches_per_minute: Some(20),
            provider_calls_per_day: None,
        };
        for field in FIELDS {
            let mut copy = Settings::defaults(&config());
            copy.set_json(field, &settings.get_json(field).unwrap()).unwrap();
            assert_eq!(copy.get_json(field).unwrap(), settings.get_json(field).unwrap(), "{field}");
            let mut from_pb = Settings::defaults(&config());
            if *field == "gifs" {
                // The key never travels in settings; the receiver keeps its own.
                from_pb.gifs = crate::gifs::Setup {
                    provider: settings.gifs.provider,
                    api_key: settings.gifs.api_key.clone(),
                    ..Default::default()
                };
            }
            from_pb.set_from_pb(field, &settings.to_pb()).unwrap();
            assert_eq!(from_pb.get_json(field).unwrap(), settings.get_json(field).unwrap(), "{field} via pb");
        }
    }

    #[test]
    fn admins_never_get_the_turn_secret() {
        let mut settings = Settings::defaults(&config());
        settings.turn_secret = "a-long-turn-secret-0123".into();
        let shown = settings.to_admin_pb();
        assert!(shown.turn_secret.is_empty());
        assert!(shown.turn_secret_set);
        assert_eq!(shown.turn_secret_hint, "0123");
        // Shards still get it, to make each caller's TURN password.
        assert_eq!(settings.to_pb().turn_secret, settings.turn_secret);
        settings.turn_secret = "short".into();
        assert!(settings.to_admin_pb().turn_secret_hint.is_empty());
        settings.turn_secret.clear();
        assert!(!settings.to_admin_pb().turn_secret_set);
    }

    #[test]
    fn admins_never_get_the_gif_key() {
        let mut settings = Settings::defaults(&config());
        settings.gifs = crate::gifs::Setup {
            provider: crate::gifs::Kind::Giphy,
            api_key: "giphy-key-abcd".into(),
            ..Default::default()
        };
        let shown = settings.to_admin_pb().gifs.unwrap();
        assert!(shown.api_key.is_empty());
        assert!(shown.api_key_set);
        assert_eq!(shown.api_key_hint, "abcd");
        // Saving the page as shown (no key) keeps the key.
        let mut saved = settings.clone();
        saved.set_from_pb("gifs", &settings.to_admin_pb()).unwrap();
        assert_eq!(saved.gifs.api_key, "giphy-key-abcd");
        // Nor does it go to gateways and shards.
        assert!(settings.to_pb().gifs.unwrap().api_key.is_empty());
        // Switching provider never carries the key over.
        let mut switched = settings.to_admin_pb();
        switched.gifs.as_mut().unwrap().provider = pb::GifProvider::Klipy as i32;
        saved.set_from_pb("gifs", &switched).unwrap();
        assert!(saved.gifs.api_key.is_empty());
        assert!(saved.set_json("gifs", &serde_json::json!({"provider": "tenor"})).is_err());
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
        assert!(s.set_json("ice_urls", &serde_json::json!(["https://turn.example.com"])).is_err());
        assert!(s.set_json("ice_urls", &serde_json::json!(["turn:turn.example.com:3478?transport=tcp"])).is_ok());
        assert!(expand(&["nope".into()]).is_err());
        assert_eq!(expand(&["default_limits".into()]).unwrap().len(), 6);
        assert!(s.set_json("call_recordings_keep_days", &Value::from(0)).is_err());
        assert!(s.set_json("call_recordings_keep_days", &Value::from(30)).is_ok());
        assert!(s.set_json("call_recordings_keep_days", &Value::Null).is_ok());
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
