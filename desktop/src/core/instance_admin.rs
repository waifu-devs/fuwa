//! Running an instance: the settings its admins change while it runs (each
//! starting from the operator's environment), and the moderation services
//! servers' AutoMod can ask. The calls behind instance settings, as in the
//! web app's `InstanceSettingsDialog.tsx`.

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// The scam the providers are tried with: no one's words.
pub const SAMPLE: &str = "Free nitro for everyone who logs in at discord-gift.example with their password!";

/// At most this many of the admins' own providers (the instance's limit).
pub const MAX_CUSTOM: usize = 8;

/// What fuwa knows about a provider it ships with, beyond what the instance sends.
pub struct Known {
    pub name: &'static str,
    pub host: &'static str,
    pub blurb: &'static str,
    /// (id, label, hint); the first is the default.
    pub models: &'static [(&'static str, &'static str, &'static str)],
    pub key_help: &'static str,
    /// The hue its badge is drawn in.
    pub hue: f32,
}

pub fn known(id: &str) -> Option<&'static Known> {
    match id {
        "typesafe-jev" => Some(&JEV),
        "cloudflare-clef" => Some(&CLEF),
        _ => None,
    }
}

static JEV: Known = Known {
    name: "TypeSafe Jev",
    host: "api.typesafe.ai",
    blurb: "A decision model that answers yes-or-no questions about a text with a calibrated probability. Text only, \
            run in the US.",
    models: &[("jev-latest", "Jev", "The current release"), ("jev-preview", "Jev preview", "The next release, early")],
    key_help: "An API key from your TypeSafe account. Billed per word read, by TypeSafe.",
    hue: 0.6,
};

static CLEF: Known = Known {
    name: "Cloudflare Clef",
    host: "api.cloudflare.com",
    blurb: "Cloudflare's open decision models on Workers AI, answering the same questions as Jev. Runs on \
            Cloudflare's network.",
    models: &[
        ("@cf/cloudflare/clef", "Clef", "Most accurate"),
        ("@cf/cloudflare/clef-flash", "Clef flash", "Fastest, cheapest"),
    ],
    key_help: "An API token with Workers AI Read, either an account token (Manage Account, Account API Tokens) \
               or a user token (My Profile, API Tokens), and your account id from the dashboard's sidebar. Billed \
               by Cloudflare.",
    hue: 0.07,
};

/// The admins' own providers ("custom" until the instance gives them an id).
pub fn is_custom(p: &pb::AutoModProviderSettings) -> bool {
    p.id == "custom" || p.id.starts_with("custom-")
}

/// Where an address points, while it's an https URL with a host.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.chars().any(char::is_whitespace) {
        return None;
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?.to_owned(),
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { authority })
            .to_owned(),
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// What a provider's card needs before it can be tried or turned on, as words, or `None` when it's ready.
/// A saved key stays with its address: a custom one moved somewhere new needs it typed again.
pub fn missing_for(p: &pb::AutoModProviderSettings, saved: Option<&pb::AutoModProviderSettings>) -> Option<String> {
    let moved = is_custom(p) && saved.is_some_and(|s| s.url.trim() != p.url.trim());
    let has_key = (p.api_key_set && !moved) || !p.api_key.trim().is_empty();
    if is_custom(p) {
        let mut need = Vec::new();
        if p.name.trim().is_empty() {
            need.push("a name");
        }
        if host_of(&p.url).is_none() {
            need.push("an https address");
        }
        if !p.header.trim().is_empty() && !has_key {
            need.push("the key");
        }
        return (!need.is_empty()).then(|| format!("Add {} to test it and turn it on.", need.join(" and ")));
    }
    let clef = p.id == "cloudflare-clef";
    let ready = has_key && (!clef || p.account_id.trim().len() == 32);
    (!ready)
        .then(|| format!("Add the {} to test it and turn it on.", if clef { "token and account id" } else { "key" }))
}

/// Whether a custom provider's saved key no longer goes with its address.
pub fn moved(p: &pb::AutoModProviderSettings, saved: Option<&pb::AutoModProviderSettings>) -> bool {
    is_custom(p) && saved.is_some_and(|s| s.url.trim() != p.url.trim())
}

fn fingerprint(p: &pb::AutoModProviderSettings) -> (String, bool, String, String, String, String, String, String) {
    (
        p.id.clone(),
        p.enabled,
        p.api_key.trim().to_owned(),
        p.model.trim().to_owned(),
        p.account_id.trim().to_ascii_lowercase(),
        p.name.trim().to_owned(),
        p.url.trim().to_owned(),
        p.header.trim().to_ascii_lowercase(),
    )
}

/// A list of addresses as typed, one per line: blank lines don't count.
fn lines(list: &[String]) -> Vec<&str> {
    list.iter().map(|o| o.trim()).filter(|o| !o.is_empty()).collect()
}

/// Every setting the desktop changes, as the API names it, in the web's order.
pub const PATHS: [&str; 46] = [
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
    "shared_channels",
    "mcp",
    "profile_effects",
    "rich_presence",
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
    "voice_message_seconds",
    "voice_message_bytes",
    "voice_message_bytes_per_day",
    "poll_votes_per_minute",
    "commands_per_minute",
    "telemetry",
    "web",
    "calls",
    "call_recordings",
    "call_recording_video",
    "call_recordings_keep_days",
    "ice_urls",
    "turn_secret",
    "automod_providers",
    "automod_checks_per_day",
    "federation",
    "federation_blocked_hosts",
    "shared_remote_sends_per_minute",
    "shared_remote_people",
    "shared_remote_file_bytes_per_day",
    "shared_file_fetches_in_flight",
];

/// A default cap, or `None` for one that isn't there.
fn limit(s: &pb::InstanceSettings, path: &str) -> Option<i64> {
    let l = s.default_limits.as_ref()?;
    match path {
        "default_limits.members" => l.members,
        "default_limits.channels" => l.channels,
        "default_limits.storage_bytes" => l.storage_bytes,
        "default_limits.attachment_bytes" => l.attachment_bytes,
        "default_limits.emojis" => l.emojis,
        "default_limits.recording_bytes" => l.recording_bytes,
        _ => None,
    }
}

/// The cap a path names, for the pages that switch caps on and off.
pub fn cap(s: &pb::InstanceSettings, path: &str) -> Option<i64> {
    match path {
        "servers_per_account" => s.servers_per_account,
        "picture_upload_bytes" => s.picture_upload_bytes,
        "picture_upload_bytes_per_day" => s.picture_upload_bytes_per_day,
        "call_recordings_keep_days" => s.call_recordings_keep_days,
        "automod_checks_per_day" => s.automod_checks_per_day,
        "attachment_upload_bytes" => s.attachment_upload_bytes,
        "attachment_upload_bytes_per_day" => s.attachment_upload_bytes_per_day,
        "voice_message_seconds" => s.voice_message_seconds,
        "voice_message_bytes" => s.voice_message_bytes,
        "voice_message_bytes_per_day" => s.voice_message_bytes_per_day,
        "poll_votes_per_minute" => s.poll_votes_per_minute,
        "commands_per_minute" => s.commands_per_minute,
        "shared_remote_sends_per_minute" => s.shared_remote_sends_per_minute,
        "shared_remote_people" => s.shared_remote_people,
        "shared_remote_file_bytes_per_day" => s.shared_remote_file_bytes_per_day,
        "shared_file_fetches_in_flight" => s.shared_file_fetches_in_flight,
        _ => limit(s, path),
    }
}

/// Sets the cap a path names (`None` is no limit).
pub fn set_cap(s: &mut pb::InstanceSettings, path: &str, value: Option<i64>) {
    let slot = match path {
        "servers_per_account" => &mut s.servers_per_account,
        "picture_upload_bytes" => &mut s.picture_upload_bytes,
        "picture_upload_bytes_per_day" => &mut s.picture_upload_bytes_per_day,
        "call_recordings_keep_days" => &mut s.call_recordings_keep_days,
        "automod_checks_per_day" => &mut s.automod_checks_per_day,
        "attachment_upload_bytes" => &mut s.attachment_upload_bytes,
        "attachment_upload_bytes_per_day" => &mut s.attachment_upload_bytes_per_day,
        "voice_message_seconds" => &mut s.voice_message_seconds,
        "voice_message_bytes" => &mut s.voice_message_bytes,
        "voice_message_bytes_per_day" => &mut s.voice_message_bytes_per_day,
        "poll_votes_per_minute" => &mut s.poll_votes_per_minute,
        "commands_per_minute" => &mut s.commands_per_minute,
        "shared_remote_sends_per_minute" => &mut s.shared_remote_sends_per_minute,
        "shared_remote_people" => &mut s.shared_remote_people,
        "shared_remote_file_bytes_per_day" => &mut s.shared_remote_file_bytes_per_day,
        "shared_file_fetches_in_flight" => &mut s.shared_file_fetches_in_flight,
        _ => {
            let l = s.default_limits.get_or_insert_with(Default::default);
            match path {
                "default_limits.members" => &mut l.members,
                "default_limits.channels" => &mut l.channels,
                "default_limits.storage_bytes" => &mut l.storage_bytes,
                "default_limits.attachment_bytes" => &mut l.attachment_bytes,
                "default_limits.emojis" => &mut l.emojis,
                "default_limits.recording_bytes" => &mut l.recording_bytes,
                _ => return,
            }
        }
    };
    *slot = value;
}

/// Whether one setting differs between two drafts, read as the web reads it
/// (trimmed text, blank lines dropped).
fn differs(a: &pb::InstanceSettings, b: &pb::InstanceSettings, path: &str) -> bool {
    let prints = |s: &pb::InstanceSettings| s.automod_providers.iter().map(fingerprint).collect::<Vec<_>>();
    match path {
        "name" => a.name.trim() != b.name.trim(),
        "public_url" => a.public_url.trim() != b.public_url.trim(),
        "allowed_origins" => lines(&a.allowed_origins) != lines(&b.allowed_origins),
        "local_accounts" => a.local_accounts != b.local_accounts,
        "linked_accounts" => a.linked_accounts != b.linked_accounts,
        "linked_issuer" => a.linked_issuer.trim().trim_end_matches('/') != b.linked_issuer.trim().trim_end_matches('/'),
        "sso_accounts" => a.sso_accounts != b.sso_accounts,
        "sso_provider" => {
            crate::core::sso::provider_print(a.sso_provider.as_ref())
                != crate::core::sso::provider_print(b.sso_provider.as_ref())
        }
        "server_creation" => a.server_creation != b.server_creation,
        "agent_creation" => a.agent_creation != b.agent_creation,
        "shared_channels" => a.shared_channels != b.shared_channels,
        "mcp" => a.mcp != b.mcp,
        "profile_effects" => a.profile_effects != b.profile_effects,
        "rich_presence" => a.rich_presence != b.rich_presence,
        "telemetry" => a.telemetry != b.telemetry,
        "web" => a.web != b.web,
        "calls" => a.calls != b.calls,
        "call_recordings" => a.call_recordings != b.call_recordings,
        "call_recording_video" => a.call_recording_video != b.call_recording_video,
        "ice_urls" => lines(&a.ice_urls) != lines(&b.ice_urls),
        "turn_secret" => a.turn_secret.trim() != b.turn_secret.trim(),
        "automod_providers" => prints(a) != prints(b),
        "federation" => a.federation != b.federation,
        "federation_blocked_hosts" => hosts(&a.federation_blocked_hosts) != hosts(&b.federation_blocked_hosts),
        _ => cap(a, path) != cap(b, path),
    }
}

/// Host names as the instance reads them: trimmed, lowercased, each once, in order.
/// The web's `hosts`.
pub fn hosts<S: AsRef<str>>(list: &[S]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    list.iter().map(|h| h.as_ref().trim().to_lowercase()).filter(|h| !h.is_empty() && seen.insert(h.clone())).collect()
}

/// The settings that differ between a draft and what's saved, as the API names them.
pub fn changed(draft: &pb::InstanceSettings, saved: &pb::InstanceSettings) -> Vec<String> {
    PATHS.iter().filter(|p| differs(draft, saved, p)).map(|p| (*p).to_owned()).collect()
}

/// Copies one setting from a draft into fresh settings, to keep an edit that wasn't saved.
pub fn copy_field(into: &mut pb::InstanceSettings, from: &pb::InstanceSettings, path: &str) {
    match path {
        "name" => into.name = from.name.clone(),
        "public_url" => into.public_url = from.public_url.clone(),
        "allowed_origins" => into.allowed_origins = from.allowed_origins.clone(),
        "local_accounts" => into.local_accounts = from.local_accounts,
        "linked_accounts" => into.linked_accounts = from.linked_accounts,
        "linked_issuer" => into.linked_issuer = from.linked_issuer.clone(),
        "sso_accounts" => into.sso_accounts = from.sso_accounts,
        "sso_provider" => into.sso_provider = from.sso_provider.clone(),
        "server_creation" => into.server_creation = from.server_creation,
        "agent_creation" => into.agent_creation = from.agent_creation,
        "shared_channels" => into.shared_channels = from.shared_channels,
        "mcp" => into.mcp = from.mcp,
        "profile_effects" => into.profile_effects = from.profile_effects,
        "rich_presence" => into.rich_presence = from.rich_presence,
        "telemetry" => into.telemetry = from.telemetry,
        "web" => into.web = from.web,
        "calls" => into.calls = from.calls,
        "call_recordings" => into.call_recordings = from.call_recordings,
        "call_recording_video" => into.call_recording_video = from.call_recording_video,
        "ice_urls" => into.ice_urls = from.ice_urls.clone(),
        // The instance never sends the secret back, only whether one is saved and its end.
        "turn_secret" => {
            into.turn_secret = from.turn_secret.clone();
            into.turn_secret_set = from.turn_secret_set;
            into.turn_secret_hint = from.turn_secret_hint.clone();
        }
        "automod_providers" => into.automod_providers = from.automod_providers.clone(),
        "federation" => into.federation = from.federation,
        "federation_blocked_hosts" => into.federation_blocked_hosts = from.federation_blocked_hosts.clone(),
        _ => set_cap(into, path, cap(from, path)),
    }
}

/// Whether a sign-in provider can send people back to this address: https, or http on this computer.
/// The web's `canReturnTo`.
pub fn can_return_to(url: &str) -> bool {
    let url = url.trim();
    if host_of(url).is_some() {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => format!("[{}]", v6.split(']').next().unwrap_or_default()),
        None => authority.split(':').next().unwrap_or_default().to_owned(),
    };
    ["localhost", "127.0.0.1", "[::1]"].contains(&host.to_ascii_lowercase().as_str())
}

/// Whether an identity provider is filled in enough to sign people in. The web's `providerReady`.
pub fn provider_ready(p: Option<&pb::IdentityProvider>) -> bool {
    let Some(p) = p.filter(|p| !p.name.trim().is_empty()) else { return false };
    if p.protocol == pb::SsoProtocol::Oidc as i32 {
        return p.oidc.as_ref().is_some_and(|o| {
            !o.issuer.trim().is_empty()
                && !o.client_id.trim().is_empty()
                && (!o.client_secret.is_empty() || o.client_secret_set)
        });
    }
    if p.protocol == pb::SsoProtocol::Saml as i32 {
        return p.saml.as_ref().is_some_and(|s| {
            !s.entity_id.trim().is_empty()
                && !s.sso_url.trim().is_empty()
                && s.certificates.contains("BEGIN CERTIFICATE")
        });
    }
    false
}

/// Whether signing in with waifu.dev would work with these settings: on, with an https public address.
pub fn linked_works(s: &pb::InstanceSettings) -> bool {
    s.linked_accounts != pb::LinkedAccounts::Off as i32 && can_return_to(&s.public_url)
}

/// Whether single sign-on would work with these settings: on, set up, with an https public address.
pub fn sso_works(s: &pb::InstanceSettings) -> bool {
    (s.sso_accounts == pb::SsoAccounts::Open as i32 || s.sso_accounts == pb::SsoAccounts::Closed as i32)
        && provider_ready(s.sso_provider.as_ref())
        && can_return_to(&s.public_url)
}

/// The units a size cap is typed in.
pub const UNITS: [(&str, i64); 3] = [("MB", 1 << 20), ("GB", 1 << 30), ("TB", 1 << 40)];

/// A size as a number and the unit it reads best in (GB when there's none), as the web's `splitBytes`.
pub fn split_bytes(bytes: Option<i64>) -> (String, usize) {
    let Some(n) = bytes else { return (String::new(), 1) };
    let unit = (0..UNITS.len())
        .rev()
        .find(|&u| {
            let hundredths = n as f64 / UNITS[u].1 as f64 * 100.0;
            n >= UNITS[u].1 && hundredths.fract() == 0.0
        })
        .unwrap_or(0);
    let amount = (n as f64 / UNITS[unit].1 as f64 * 100.0).round() / 100.0;
    (format!("{amount}"), unit)
}

/// What's typed in a cap's box as a number (in `unit` for sizes), or `None` while it isn't one.
pub fn parse_cap(text: &str, unit: Option<usize>) -> Option<i64> {
    let n: f64 = text.trim().parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let factor = unit.map_or(1, |u| UNITS[u.min(UNITS.len() - 1)].1);
    let value = (n * factor as f64).round();
    (value <= i64::MAX as f64).then_some(value as i64)
}

/// A size in words, like the web's `formatBytes`.
pub fn format_bytes(bytes: i64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let (mut value, mut unit) = (bytes as f64 / 1024.0, 0);
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 { format!("{value:.0} {}", units[unit]) } else { format!("{value:.1} {}", units[unit]) }
}

/// A count cap in words: "no limit" when it's off.
pub fn count_label(n: Option<i64>) -> String {
    n.map_or_else(|| "no limit".to_owned(), group_digits)
}

/// A cap counted per minute in words: "no limit" when it's off.
pub fn per_minute_label(n: Option<i64>) -> String {
    n.map_or_else(|| "no limit".to_owned(), |n| format!("{} a minute", group_digits(n)))
}

/// Where a count cap starts when it's switched on: the web's placeholder.
pub fn starting_cap(path: &str) -> i64 {
    match path {
        "poll_votes_per_minute" => 30,
        "commands_per_minute" => 20,
        "shared_file_fetches_in_flight" => 8,
        "shared_remote_sends_per_minute" => 120,
        "shared_remote_people" => 500,
        "voice_message_seconds" => 300,
        _ => 100,
    }
}

/// A size cap in words: "no limit" when it's off.
pub fn size_label(n: Option<i64>) -> String {
    n.map_or_else(|| "no limit".to_owned(), format_bytes)
}

fn group_digits(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// A label's id as words: "self_harm" reads "Self harm".
pub fn label_name(id: &str) -> String {
    let mut out = id.replace('_', " ");
    if let Some(first) = out.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    out
}

impl Core {
    /// This instance as other instances see it, and the instances it knows. Admins only.
    pub async fn federation(&self, key: &str) -> Result<pb::GetFederationResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.admin(), get_federation(pb::GetFederationRequest {})).await
    }

    /// Reaches another instance with a signed greeting and back, pinning its key here.
    pub async fn check_instance(&self, key: &str, address: &str) -> Result<pb::CheckInstanceResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.admin(), check_instance(pb::CheckInstanceRequest { address: address.trim().to_owned() })).await
    }

    /// The instance's settings, where each comes from, and how it was started. Admins only.
    pub async fn instance_settings(&self, key: &str) -> Result<pb::InstanceConfig, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.admin(), get_settings(pb::GetSettingsRequest {})).await?;
        res.config.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no settings."))
    }

    /// Saves the settings named in `update` from `settings`, and puts the ones in `reset` back to their defaults.
    pub async fn update_instance_settings(
        &self,
        key: &str,
        settings: pb::InstanceSettings,
        update: Vec<String>,
        reset: Vec<String>,
    ) -> Result<pb::InstanceConfig, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.admin(),
            update_settings(pb::UpdateSettingsRequest {
                settings: Some(settings),
                update_mask: Some(prost_types::FieldMask { paths: update }),
                reset_mask: Some(prost_types::FieldMask { paths: reset }),
            })
        )
        .await?;
        res.config.ok_or_else(|| Problem::new(Code::Internal, "The instance sent no settings."))
    }

    /// Asks a provider, saved or not, about [`SAMPLE`]. The text goes to the provider's host, from the instance.
    pub async fn test_automod_provider(
        &self,
        key: &str,
        provider: pb::AutoModProviderSettings,
    ) -> Result<pb::TestAutoModProviderResponse, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.admin(),
            test_auto_mod_provider(pb::TestAutoModProviderRequest {
                provider: Some(provider),
                content: SAMPLE.to_owned()
            })
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn blocked_hosts_read_like_the_web() {
        assert_eq!(
            super::hosts(&[" Spam.Example.com", "", "spam.example.com", "b.org"]),
            ["spam.example.com", "b.org"]
        );
    }

    use super::*;

    fn custom(url: &str) -> pb::AutoModProviderSettings {
        pb::AutoModProviderSettings { id: "custom".into(), name: "Ours".into(), url: url.into(), ..Default::default() }
    }

    #[test]
    fn an_empty_turn_secret_keeps_the_saved_one() {
        let saved =
            pb::InstanceSettings { turn_secret_set: true, turn_secret_hint: "0123".into(), ..Default::default() };
        let mut draft = saved.clone();
        assert!(changed(&draft, &saved).is_empty());
        draft.turn_secret = "a-new-turn-secret".into();
        assert_eq!(changed(&draft, &saved), ["turn_secret"]);
        let mut back = pb::InstanceSettings::default();
        copy_field(&mut back, &saved, "turn_secret");
        assert!(back.turn_secret_set && back.turn_secret_hint == "0123" && back.turn_secret.is_empty());
    }

    #[test]
    fn only_https_addresses_have_a_host() {
        assert_eq!(host_of("https://Mod.Example.com/v1/check").as_deref(), Some("mod.example.com"));
        assert_eq!(host_of(" https://mod.example.com:8443 ").as_deref(), Some("mod.example.com"));
        assert_eq!(host_of("https://[2001:db8::1]:8443/x").as_deref(), Some("2001:db8::1"));
        for bad in ["http://mod.example.com/", "mod.example.com", "https://", "https://user@mod.example.com/", ""] {
            assert_eq!(host_of(bad), None, "{bad}");
        }
    }

    #[test]
    fn cards_say_what_they_still_need() {
        let jev = pb::AutoModProviderSettings { id: "typesafe-jev".into(), ..Default::default() };
        assert_eq!(missing_for(&jev, None).as_deref(), Some("Add the key to test it and turn it on."));
        let typed = pb::AutoModProviderSettings { api_key: "k".into(), ..jev.clone() };
        assert_eq!(missing_for(&typed, None), None);
        let clef =
            pb::AutoModProviderSettings { id: "cloudflare-clef".into(), api_key_set: true, ..Default::default() };
        assert_eq!(
            missing_for(&clef, None).as_deref(),
            Some("Add the token and account id to test it and turn it on.")
        );
        let clef = pb::AutoModProviderSettings { account_id: "0123456789abcdef0123456789abcdef".into(), ..clef };
        assert_eq!(missing_for(&clef, None), None);

        assert_eq!(missing_for(&custom("https://a.example/"), None), None);
        let nameless = pb::AutoModProviderSettings { name: " ".into(), ..custom("ftp://a") };
        assert_eq!(
            missing_for(&nameless, None).as_deref(),
            Some("Add a name and an https address to test it and turn it on.")
        );
        // A saved key stays with the address it was saved for.
        let saved =
            pb::AutoModProviderSettings { api_key_set: true, header: "X-Key".into(), ..custom("https://a.example/") };
        assert_eq!(missing_for(&saved, Some(&saved)), None);
        let moved_away = pb::AutoModProviderSettings { url: "https://b.example/".into(), ..saved.clone() };
        assert!(moved(&moved_away, Some(&saved)));
        assert_eq!(missing_for(&moved_away, Some(&saved)).as_deref(), Some("Add the key to test it and turn it on."));
    }

    #[test]
    fn changes_are_named_like_the_api() {
        let saved = pb::InstanceSettings {
            automod_providers: vec![pb::AutoModProviderSettings { id: "typesafe-jev".into(), ..Default::default() }],
            allowed_origins: vec!["https://a.example".into()],
            ..Default::default()
        };
        let mut draft = saved.clone();
        assert!(changed(&draft, &saved).is_empty());
        draft.automod_providers[0].header = "  ".into();
        draft.allowed_origins.push("  ".into());
        draft.name = "  ".into();
        assert!(changed(&draft, &saved).is_empty(), "blanks aren't changes");
        draft.automod_providers[0].enabled = true;
        draft.telemetry = true;
        set_cap(&mut draft, "default_limits.emojis", Some(50));
        assert_eq!(changed(&draft, &saved), ["default_limits.emojis", "telemetry", "automod_providers"]);

        // A save keeps the edits it didn't send.
        let mut fresh = saved.clone();
        for path in changed(&draft, &saved) {
            copy_field(&mut fresh, &draft, &path);
        }
        assert!(changed(&fresh, &draft).is_empty());
        assert_eq!(label_name("self_harm"), "Self harm");
    }

    #[test]
    fn sign_ins_come_back_only_to_https_or_this_computer() {
        for good in ["https://fuwa.example", "http://localhost:5173/", "http://127.0.0.1", "http://[::1]:8080"] {
            assert!(can_return_to(good), "{good}");
        }
        for bad in ["http://fuwa.example", "http://localhost.example", "", "fuwa.example"] {
            assert!(!can_return_to(bad), "{bad}");
        }
        let mut s = pb::InstanceSettings {
            linked_accounts: pb::LinkedAccounts::Open as i32,
            public_url: "http://fuwa.example".into(),
            ..Default::default()
        };
        assert!(!linked_works(&s));
        s.public_url = "https://fuwa.example".into();
        assert!(linked_works(&s));
        assert!(!sso_works(&s), "no provider yet");
        s.sso_accounts = pb::SsoAccounts::Open as i32;
        s.sso_provider = Some(pb::IdentityProvider {
            protocol: pb::SsoProtocol::Oidc as i32,
            name: "Acme".into(),
            oidc: Some(pb::OidcProvider {
                issuer: "https://id.acme.example".into(),
                client_id: "fuwa".into(),
                client_secret_set: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(sso_works(&s));
    }

    #[test]
    fn caps_read_like_the_web() {
        assert_eq!(split_bytes(None), (String::new(), 1));
        assert_eq!(split_bytes(Some(5 << 30)), ("5".to_owned(), 1));
        assert_eq!(split_bytes(Some(512 << 20)), ("512".to_owned(), 0));
        assert_eq!(split_bytes(Some(3 << 40)), ("3".to_owned(), 2));
        assert_eq!(split_bytes(Some(1536 << 20)), ("1.5".to_owned(), 1));
        assert_eq!(parse_cap("1.5", Some(1)), Some(1536 << 20));
        assert_eq!(parse_cap(" 250 ", None), Some(250));
        for bad in ["", "-1", "ten", "NaN"] {
            assert_eq!(parse_cap(bad, None), None, "{bad}");
        }
        assert_eq!(count_label(Some(1_234_567)), "1,234,567");
        assert_eq!(size_label(Some(5 << 30)), "5.0 GB");
        assert_eq!(size_label(Some(25 << 30)), "25 GB");
        assert_eq!(size_label(None), "no limit");
    }
}
