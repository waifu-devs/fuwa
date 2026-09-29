use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ulid::{Generator, Ulid};

use crate::error::{Error, Result};

static GENERATOR: Mutex<Option<Generator>> = Mutex::new(None);

/// A new ULID. Ids sort by creation time, and ids made in the same millisecond
/// still sort in the order they were made.
pub fn new_id() -> String {
    let mut generator = GENERATOR.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let generator = generator.get_or_insert_with(Generator::new);
    generator.generate().unwrap_or_else(|_| Ulid::new()).to_string()
}

/// Checks an id from a request and returns it in canonical form. Server ids name
/// files on disk, so nothing else gets through.
pub fn parse_id(what: &'static str, value: &str) -> Result<String> {
    Ulid::from_string(value)
        .map(|id| id.to_string())
        .map_err(|_| Error::InvalidArgument(format!("{what} is not a valid id")))
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

pub fn timestamp(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp { seconds: ms.div_euclid(1000), nanos: (ms.rem_euclid(1000) * 1_000_000) as i32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sort_in_creation_order() {
        let ids: Vec<String> = (0..1000).map(|_| new_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn parse_id_rejects_paths() {
        assert!(parse_id("server_id", "../../etc/passwd").is_err());
        assert!(parse_id("server_id", "").is_err());
        let id = new_id();
        assert_eq!(parse_id("server_id", &id.to_lowercase()).unwrap(), id);
    }
}
