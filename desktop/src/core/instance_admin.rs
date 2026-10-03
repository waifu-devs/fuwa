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
    key_help: "An API token with Workers AI permission (Cloudflare dashboard, My Profile, API Tokens), and your \
               account id from the dashboard's sidebar. Billed by Cloudflare.",
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

/// The settings that differ between a draft and what's saved, as the API names them.
/// Only the ones the desktop shows so far.
pub fn changed(draft: &pb::InstanceSettings, saved: &pb::InstanceSettings) -> Vec<String> {
    let mut out = Vec::new();
    if draft.telemetry != saved.telemetry {
        out.push("telemetry".to_owned());
    }
    let prints = |s: &pb::InstanceSettings| s.automod_providers.iter().map(fingerprint).collect::<Vec<_>>();
    if prints(draft) != prints(saved) {
        out.push("automod_providers".to_owned());
    }
    out
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
    use super::*;

    fn custom(url: &str) -> pb::AutoModProviderSettings {
        pb::AutoModProviderSettings { id: "custom".into(), name: "Ours".into(), url: url.into(), ..Default::default() }
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
            ..Default::default()
        };
        let mut draft = saved.clone();
        assert!(changed(&draft, &saved).is_empty());
        draft.automod_providers[0].header = "  ".into();
        assert!(changed(&draft, &saved).is_empty(), "blanks aren't changes");
        draft.automod_providers[0].enabled = true;
        draft.telemetry = true;
        assert_eq!(changed(&draft, &saved), ["telemetry", "automod_providers"]);
        assert_eq!(label_name("self_harm"), "Self harm");
    }
}
