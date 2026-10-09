//! Compatibility dates: which features this instance has, and since when.
//!
//! `proto/fuwa/v1/features.json` lists every feature an app has to know to
//! use, each with the date it arrived. This instance advertises the list,
//! its own date (the newest feature's) and the oldest app date it serves
//! fully in `Node.versions`, so an older app can say "update to use this"
//! where it would otherwise show something broken, and keeps working
//! everywhere else. Nothing is refused or forced (docs/compatibility.md).

use std::sync::LazyLock;

use serde::Deserialize;

use crate::pb;

#[derive(Deserialize)]
struct List {
    min_client_date: String,
    features: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    date: String,
    title: String,
}

static LIST: LazyLock<List> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../proto/fuwa/v1/features.json")).expect("features.json is valid")
});

/// This build's compatibility date: the newest feature's.
pub fn date() -> &'static str {
    LIST.features.iter().map(|f| f.date.as_str()).max().unwrap_or_default()
}

/// What `Node.versions` says, with the newer release when one is known.
pub fn versions(newer_release: Option<pb::Release>) -> pb::Versions {
    pb::Versions {
        newer_release,
        compatibility_date: date().into(),
        min_client_date: LIST.min_client_date.clone(),
        features: LIST
            .features
            .iter()
            .map(|f| pb::Feature { id: f.id.clone(), date: f.date.clone(), title: f.title.clone() })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_date(s: &str) -> bool {
        let b = s.as_bytes();
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    }

    #[test]
    fn the_feature_list_is_well_formed() {
        assert!(is_date(&LIST.min_client_date));
        assert!(LIST.min_client_date.as_str() <= date(), "apps can't be asked for a date no feature has");
        let mut seen = std::collections::HashSet::new();
        let mut last = "";
        for f in &LIST.features {
            assert!(is_date(&f.date), "{}: {}", f.id, f.date);
            assert!(!f.title.is_empty() && !f.id.is_empty());
            assert!(f.title.chars().count() <= 40, "{}: titles fit in 40 characters", f.id);
            assert!(seen.insert(f.id.as_str()), "{} twice", f.id);
            assert!(f.date.as_str() >= last, "{}: features are added at the end, in date order", f.id);
            last = &f.date;
        }
        let v = versions(None);
        assert_eq!(v.compatibility_date, date());
        assert_eq!(v.features.len(), LIST.features.len());
    }
}
