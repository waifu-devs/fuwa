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

/// A timestamp from the wire, as milliseconds since the Unix epoch.
pub fn millis(t: &prost_types::Timestamp) -> i64 {
    t.seconds.saturating_mul(1000).saturating_add(i64::from(t.nanos) / 1_000_000)
}

/// A length of time as people say it, in its largest whole unit, rounded up:
/// "1 minute", "3 days".
pub fn span(ms: i64) -> String {
    let seconds = (ms.max(0) + 999) / 1000;
    let (count, unit) = [(86_400, "day"), (3_600, "hour"), (60, "minute"), (1, "second")]
        .into_iter()
        .find(|(size, _)| seconds >= *size)
        .map(|(size, unit)| ((seconds + size - 1) / size, unit))
        .unwrap_or((0, "second"));
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_read_like_people_say_them() {
        assert_eq!(span(0), "0 seconds");
        assert_eq!(span(1), "1 second");
        assert_eq!(span(60_000), "1 minute");
        assert_eq!(span(61_000), "2 minutes");
        assert_eq!(span(7 * 86_400_000), "7 days");
    }

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
