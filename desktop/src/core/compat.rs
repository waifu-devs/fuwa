//! Compatibility dates (docs/compatibility.md). `proto/fuwa/v1/features.json`
//! lists every feature an app has to know to use, each with the date it
//! arrived, and is built into this app, so it knows exactly the features it
//! was made with. An instance says which it has in `Node.versions`. Where an
//! instance has a feature this app doesn't know, the app says "update to use
//! this" instead of showing something broken, and everything else keeps
//! working. Nothing is forced.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::pb;

#[derive(Debug, Clone, Deserialize)]
pub struct Feature {
    pub id: String,
    pub date: String,
    pub title: String,
}

#[derive(Deserialize)]
struct List {
    features: Vec<Feature>,
}

/// The features this build knows.
pub static FEATURES: LazyLock<Vec<Feature>> = LazyLock::new(|| {
    serde_json::from_str::<List>(include_str!("../../../proto/fuwa/v1/features.json"))
        .expect("features.json is valid")
        .features
});

/// Instances from before compatibility dates had every feature dated this day or earlier.
const BASELINE: &str = "2026-10-04";

/// This build's compatibility date: its newest feature's.
pub fn client_date() -> &'static str {
    FEATURES.iter().map(|f| f.date.as_str()).max().unwrap_or_default()
}

/// Features the instance has that this app doesn't know: they need a newer app.
pub fn missing<'a>(versions: Option<&'a pb::Versions>, ours: &[Feature]) -> Vec<&'a pb::Feature> {
    let Some(versions) = versions else { return vec![] };
    versions.features.iter().filter(|f| !ours.iter().any(|o| o.id == f.id)).collect()
}

/// Whether this app is older than the instance serves fully.
pub fn too_old(versions: Option<&pb::Versions>, client_date: &str) -> bool {
    versions.is_some_and(|v| !v.min_client_date.is_empty() && client_date < v.min_client_date.as_str())
}

/// Whether the instance has a feature this app knows, so its screens may
/// show. An instance from before compatibility dates has the baseline ones.
pub fn instance_has(versions: Option<&pb::Versions>, id: &str, ours: &[Feature]) -> bool {
    match versions {
        Some(v) => v.features.iter().any(|f| f.id == id),
        None => ours.iter().any(|f| f.id == id && f.date.as_str() <= BASELINE),
    }
}

/// The most of a feature title or instance name shown.
const MAX_SHOWN: usize = 40;

/// A title or name from an instance, made safe to show in an update notice:
/// letters, digits, spaces and a little punctuation, never anything that
/// reads as a link or an address, and short.
pub fn shown(text: &str, fallback: &str) -> String {
    let plain: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() || " '&(),-".contains(c) { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if plain.is_empty() {
        return fallback.to_owned();
    }
    if plain.chars().count() > MAX_SHOWN {
        let cut: String = plain.chars().take(MAX_SHOWN - 1).collect();
        return format!("{}…", cut.trim_end());
    }
    plain
}

/// What an app that needs updating tells people, naming the instance it's
/// about, or None when it's fine.
pub fn update_line(versions: Option<&pb::Versions>, instance: &str) -> Option<String> {
    let name = shown(instance, "An instance");
    let need = missing(versions, &FEATURES);
    match need.as_slice() {
        [] if too_old(versions, client_date()) => Some(format!("{name} needs a newer fuwa for everything to work")),
        [] => None,
        [one] => Some(format!("{name} has {}. Update fuwa to use it", shown(&one.title, "something new"))),
        [first, rest @ ..] => Some(format!(
            "{name} has {} and {} more. Update fuwa to use them",
            shown(&first.title, "something new"),
            rest.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(id: &str, date: &str) -> pb::Feature {
        pb::Feature { id: id.into(), date: date.into(), title: id.to_uppercase() }
    }

    fn ours() -> Vec<Feature> {
        vec![
            Feature { id: "a".into(), date: "2026-10-04".into(), title: "A".into() },
            Feature { id: "b".into(), date: "2026-11-01".into(), title: "B".into() },
        ]
    }

    #[test]
    fn a_newer_instances_unknown_features_need_an_update() {
        let newer = pb::Versions {
            compatibility_date: "2026-12-01".into(),
            min_client_date: "2026-10-04".into(),
            features: vec![feature("a", "2026-10-04"), feature("b", "2026-11-01"), feature("c", "2026-12-01")],
            newer_release: None,
        };
        let need: Vec<_> = missing(Some(&newer), &ours()).iter().map(|f| f.id.clone()).collect();
        assert_eq!(need, ["c"]);
        assert!(!too_old(Some(&newer), "2026-11-01"));
        assert!(too_old(Some(&pb::Versions { min_client_date: "2026-12-01".into(), ..newer.clone() }), "2026-11-01"));
        assert!(missing(None, &ours()).is_empty());
    }

    #[test]
    fn features_show_only_where_the_instance_has_them() {
        let older = pb::Versions { features: vec![feature("a", "2026-10-04")], ..Default::default() };
        assert!(instance_has(Some(&older), "a", &ours()));
        assert!(!instance_has(Some(&older), "b", &ours()));
        assert!(instance_has(None, "a", &ours()));
        assert!(!instance_has(None, "b", &ours()));
    }

    #[test]
    fn the_line_names_the_instance_and_what_needs_the_update() {
        let all: Vec<pb::Feature> = FEATURES.iter().map(|f| feature(&f.id, &f.date)).collect();
        let with = |extra: &[&str], min: &str| pb::Versions {
            min_client_date: min.into(),
            features: all.iter().cloned().chain(extra.iter().map(|id| feature(id, "2099-01-01"))).collect(),
            ..Default::default()
        };
        let line = |v: pb::Versions| update_line(Some(&v), "Waifu Devs");
        assert_eq!(line(with(&[], "")), None);
        assert_eq!(line(with(&["x"], "")).as_deref(), Some("Waifu Devs has X. Update fuwa to use it"));
        assert_eq!(
            line(with(&["x", "y"], "")).as_deref(),
            Some("Waifu Devs has X and 1 more. Update fuwa to use them")
        );
        assert_eq!(
            line(with(&[], "2099-01-01")).as_deref(),
            Some("Waifu Devs needs a newer fuwa for everything to work")
        );
        assert!(!client_date().is_empty());
    }

    #[test]
    fn what_an_instance_says_is_shown_short_and_plain() {
        assert_eq!(
            shown("Update fuwa: download the fix at evil.example/x", "?"),
            "Update fuwa download the fix at evil ex…"
        );
        assert_eq!(shown("https://evil.example", "?"), "https evil example");
        assert_eq!(shown("::://", "something new"), "something new");
        assert_eq!(shown("Channels shared between servers", "?"), "Channels shared between servers");
    }
}
