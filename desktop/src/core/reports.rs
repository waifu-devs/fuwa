//! Anonymous reports of what went wrong and what was slow, as counts, so
//! Waifu Devs can find bugs and slow paths. The app counts panics, requests
//! that failed inside the instance, how long things took (starting up,
//! catching up, slow frames, requests) and how often a few features are
//! used, and every few minutes sends the total to one instance you're signed
//! in to whose own telemetry is on (`NodeService.SendReport`). That instance
//! adds it to its hourly report; nothing goes anywhere else.
//!
//! A report never holds message text, names, ids, file names or addresses:
//! only kinds of failure, places in the code, durations in buckets and
//! feature counts, with the app's version and the OS family. The setting
//! (`Prefs::share_reports`) turns all of it off, and off means nothing is
//! counted at all. The server's side is `server/src/reports.rs`.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::pb;

/// Upper bounds of the timing buckets, in milliseconds; one more bucket holds
/// anything longer. The server uses the same ones.
pub const BOUNDS_MS: [u64; 12] = [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 30000];
const BUCKETS: usize = BOUNDS_MS.len() + 1;

/// Entries of each kind one report may carry; more are left out.
pub const MAX_ENTRIES: usize = 64;

/// The first report goes a minute after starting, then one every ten.
pub const FIRST_SEND: Duration = Duration::from_secs(60);
pub const SEND_EVERY: Duration = Duration::from_secs(10 * 60);

/// A frame that took longer than this to build counts as slow.
pub const LONG_FRAME: Duration = Duration::from_millis(50);

/// The crash file stops growing here, so a panic in a loop can't fill the disk.
const CRASH_FILE_MAX: u64 = 16 * 1024;

/// How long things took, in the fixed buckets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    pub buckets: [u32; BUCKETS],
    pub sum_ms: u64,
}

impl Timing {
    fn add(&mut self, ms: u64) {
        let bucket = BOUNDS_MS.iter().position(|&bound| ms <= bound).unwrap_or(BOUNDS_MS.len());
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.sum_ms = self.sum_ms.saturating_add(ms);
    }

    fn merge(&mut self, other: &Timing) {
        for (into, from) in self.buckets.iter_mut().zip(other.buckets) {
            *into = into.saturating_add(from);
        }
        self.sum_ms = self.sum_ms.saturating_add(other.sum_ms);
    }

    fn count(&self) -> u32 {
        self.buckets.iter().fold(0u32, |sum, n| sum.saturating_add(*n))
    }
}

/// What's waiting to be sent, for the settings page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pending {
    /// Errors counted, all kinds together.
    pub errors: u32,
    /// Durations measured.
    pub timings: u32,
    /// Times a feature was used.
    pub usage: u32,
}

/// Everything counted since the last report went out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Collector {
    errors: HashMap<(String, String), u32>,
    timings: HashMap<String, Timing>,
    usage: HashMap<String, u32>,
}

impl Collector {
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty() && self.timings.is_empty() && self.usage.is_empty()
    }

    /// Counts a failure of some kind at a place in the app.
    pub fn error(&mut self, kind: &str, place: &str, count: u32) {
        let key = (label(kind, 48).into_owned(), label(place, 120).into_owned());
        if let Some(total) = self.errors.get_mut(&key) {
            *total = total.saturating_add(count);
        } else if self.errors.len() < MAX_ENTRIES {
            self.errors.insert(key, count);
        }
    }

    /// Counts how long something took.
    pub fn timing(&mut self, metric: &str, ms: u64) {
        let metric = label(metric, 96);
        if let Some(timing) = self.timings.get_mut(metric.as_ref()) {
            timing.add(ms);
        } else if self.timings.len() < MAX_ENTRIES {
            self.timings.entry(metric.into_owned()).or_default().add(ms);
        }
    }

    /// Counts a feature being used.
    pub fn used(&mut self, feature: &str, count: u32) {
        let feature = label(feature, 96);
        if let Some(total) = self.usage.get_mut(feature.as_ref()) {
            *total = total.saturating_add(count);
        } else if self.usage.len() < MAX_ENTRIES {
            self.usage.insert(feature.into_owned(), count);
        }
    }

    /// Adds a report that didn't go out back in, keeping to the limits.
    pub fn merge(&mut self, other: Collector) {
        for ((kind, place), count) in other.errors {
            self.error(&kind, &place, count);
        }
        for (metric, timing) in other.timings {
            if let Some(into) = self.timings.get_mut(&metric) {
                into.merge(&timing);
            } else if self.timings.len() < MAX_ENTRIES {
                self.timings.insert(metric, timing);
            }
        }
        for (feature, count) in other.usage {
            self.used(&feature, count);
        }
    }

    pub fn pending(&self) -> Pending {
        let sum = |values: &mut dyn Iterator<Item = u32>| values.fold(0u32, |sum, n| sum.saturating_add(n));
        Pending {
            errors: sum(&mut self.errors.values().copied()),
            timings: sum(&mut self.timings.values().map(Timing::count)),
            usage: sum(&mut self.usage.values().copied()),
        }
    }

    /// The report as it's sent: the counts, the app's version and the OS family.
    pub fn to_report(&self) -> pb::AppReport {
        let mut report = pb::AppReport {
            app: "desktop".into(),
            version: label(env!("CARGO_PKG_VERSION"), 32).into_owned(),
            platform: "native".into(),
            os: os_family().into(),
            errors: self
                .errors
                .iter()
                .map(|((kind, place), &count)| pb::ReportError { kind: kind.clone(), place: place.clone(), count })
                .collect(),
            timings: self
                .timings
                .iter()
                .map(|(metric, timing)| pb::ReportTiming {
                    metric: metric.clone(),
                    buckets: timing.buckets.to_vec(),
                    sum_ms: timing.sum_ms,
                })
                .collect(),
            usage: self
                .usage
                .iter()
                .map(|(feature, &count)| pb::ReportUsage { feature: feature.clone(), count })
                .collect(),
        };
        // Steady order, so the same counts always make the same report.
        report.errors.sort_by(|a, b| (&a.kind, &a.place).cmp(&(&b.kind, &b.place)));
        report.timings.sort_by(|a, b| a.metric.cmp(&b.metric));
        report.usage.sort_by(|a, b| a.feature.cmp(&b.feature));
        report
    }

    /// The panics held, as lines for the crash file.
    fn crash_lines(&self) -> String {
        let mut lines: Vec<String> = self
            .errors
            .iter()
            .filter(|((kind, _), _)| kind == "panic")
            .map(|((_, place), count)| format!("panic\t{place}\t{count}\n"))
            .collect();
        lines.sort();
        lines.concat()
    }

    /// Counts the panics a crash file kept.
    fn read_crash_lines(&mut self, text: &str) {
        for line in text.lines() {
            let mut parts = line.split('\t');
            let (Some("panic"), Some(place)) = (parts.next(), parts.next()) else { continue };
            let count = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
            self.error("panic", place, count);
        }
    }
}

/// Which OS family the app runs on, never more precise than that.
pub fn os_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

/// A label made safe: letters, digits and `_.:/#@-` only, at most `max`
/// characters, never empty. Borrowed when it already was.
fn label(value: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if value.is_empty() {
        return "unknown".into();
    }
    if value.len() <= max && value.chars().all(allowed) {
        return value.into();
    }
    value.chars().take(max).map(|c| if allowed(c) { c } else { '_' }).collect::<String>().into()
}

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '#' | '@' | '-')
}

/// A source file as a path inside its crate, such as "desktop/src/ui/chat.rs"
/// or "gpui-0.2.2/src/window.rs": never where it was built.
pub fn source_path(file: &str) -> String {
    let file = file.replace('\\', "/");
    // This crate's own files come relative to it.
    if let Some(inside) = file.strip_prefix("src/") {
        return label(&format!("desktop/src/{inside}"), 110).into_owned();
    }
    let trimmed = match file.rfind("/src/") {
        Some(at) => {
            let start = file[..at].rfind('/').map(|slash| slash + 1).unwrap_or(0);
            &file[start..]
        }
        None => file.rsplit('/').next().unwrap_or(&file),
    };
    label(trimmed, 110).into_owned()
}

/// The gRPC method a call names, as a metric: "rpc:MessageService/SendMessage"
/// from the service and the client's method ("send_message").
pub fn rpc_metric(service: &str, method: &str) -> Box<str> {
    let mut metric = format!("rpc:{service}/");
    for word in method.split('_') {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            metric.extend(first.to_uppercase());
            metric.push_str(chars.as_str());
        }
    }
    metric.into_boxed_str()
}

// ───────────────────────── The app's own counts ─────────────────────────

/// Whether the setting is on. Off, nothing is counted.
static ON: AtomicBool = AtomicBool::new(false);
static HELD: LazyLock<Mutex<Collector>> = LazyLock::new(Default::default);
/// Where panics are written down as they happen, so a crash that ends the
/// app is still sent next time.
static CRASH_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Starts counting (or not) for an app whose files are in `config`, picking
/// up the panics a crash left behind.
pub fn start(config: &Path, on: bool) {
    let file = config.join("crashes.txt");
    *CRASH_FILE.lock() = Some(file.clone());
    set_enabled(on);
    if on && let Ok(text) = std::fs::read_to_string(&file) {
        HELD.lock().read_crash_lines(&text);
    }
}

/// Turns counting on or off; off forgets everything held, crashes too.
pub fn set_enabled(on: bool) {
    ON.store(on, Ordering::Relaxed);
    if !on {
        *HELD.lock() = Collector::default();
        if let Some(file) = CRASH_FILE.lock().as_ref() {
            let _ = std::fs::remove_file(file);
        }
    }
}

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn error(kind: &str, place: &str) {
    if enabled() {
        HELD.lock().error(kind, place, 1);
    }
}

pub fn timing(metric: &str, took: Duration) {
    if enabled() {
        HELD.lock().timing(metric, took.as_millis().min(u128::from(u64::MAX)) as u64);
    }
}

pub fn used(feature: &str) {
    if enabled() {
        HELD.lock().used(feature, 1);
    }
}

/// What's waiting to go out.
pub fn pending() -> Pending {
    HELD.lock().pending()
}

/// Takes what's held to send it; None when there's nothing.
pub fn take() -> Option<Collector> {
    let mut held = HELD.lock();
    (!held.is_empty()).then(|| std::mem::take(&mut *held))
}

/// A report that didn't go out comes back, to try again with the next one.
pub fn put_back(report: Collector) {
    if enabled() {
        HELD.lock().merge(report);
    }
}

/// A report went out: the crash file keeps only the panics counted since.
pub fn sent() {
    let lines = HELD.lock().crash_lines();
    if let Some(file) = CRASH_FILE.lock().as_ref() {
        if lines.is_empty() {
            let _ = std::fs::remove_file(file);
        } else {
            let _ = crate::core::vault::write_file(file, lines.as_bytes());
        }
    }
}

/// Counts panics by where they happened and writes them down, then lets the
/// usual hook print them.
pub fn catch_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if enabled() {
            let place = info
                .location()
                .map(|at| format!("{}:{}", source_path(at.file()), at.line()))
                .unwrap_or_else(|| "unknown".into());
            // A panic while the counts are locked must not wait on them here.
            if let Some(mut held) = HELD.try_lock() {
                held.error("panic", &place, 1);
            }
            if let Some(file) = CRASH_FILE.try_lock().and_then(|file| file.clone()) {
                note_crash(&file, &place);
            }
        }
        previous(info);
    }));
}

fn note_crash(file: &Path, place: &str) {
    if std::fs::metadata(file).is_ok_and(|m| m.len() >= CRASH_FILE_MAX) {
        return;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    if let Ok(mut out) = options.open(file) {
        let _ = writeln!(out, "panic\t{}\t1", label(place, 120));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_land_in_their_buckets() {
        let mut held = Collector::default();
        for ms in [0, 5, 6, 50, 51, 30000, 30001, u64::MAX] {
            held.timing("startup", ms);
        }
        let timing = held.timings["startup"];
        assert_eq!(timing.buckets.len(), 13);
        assert_eq!(timing.buckets[0], 2, "0 and 5 ms");
        assert_eq!(timing.buckets[1], 1, "6 ms");
        assert_eq!(timing.buckets[3], 1, "50 ms");
        assert_eq!(timing.buckets[4], 1, "51 ms");
        assert_eq!(timing.buckets[11], 1, "30 s");
        assert_eq!(timing.buckets[12], 2, "longer");
        assert_eq!(timing.sum_ms, u64::MAX, "the sum stops at the top");
        assert_eq!(held.pending(), Pending { errors: 0, timings: 8, usage: 0 });
    }

    #[test]
    fn each_kind_holds_at_most_64() {
        let mut held = Collector::default();
        for n in 0..100 {
            held.error("rpc_internal", &format!("Service/Method{n}"), 1);
            held.timing(&format!("metric{n}"), 10);
            held.used(&format!("feature{n}"), 1);
        }
        // Ones already held still count.
        held.used("feature0", 4);
        held.error("rpc_internal", "Service/Method99", 1);
        let report = held.to_report();
        assert_eq!((report.errors.len(), report.timings.len(), report.usage.len()), (64, 64, 64));
        assert_eq!(report.usage.iter().find(|u| u.feature == "feature0").unwrap().count, 5);
        assert!(!report.errors.iter().any(|e| e.place == "Service/Method99"));
        assert!(report.timings.iter().all(|t| t.buckets.len() == 13));
    }

    #[test]
    fn labels_keep_to_the_rules() {
        assert_eq!(label("message.send", 96), "message.send");
        assert_eq!(label("chat with juan@example.com", 120), "chat_with_juan@example.com");
        assert_eq!(label("ünïcode/ok", 120), "_n_code/ok");
        assert_eq!(label("", 48), "unknown");
        assert_eq!(label(&"x".repeat(200), 48).len(), 48);
        let mut held = Collector::default();
        held.error("Bad kind!", "a place\nwith lines", 1);
        let report = held.to_report();
        assert_eq!(
            (report.errors[0].kind.as_str(), report.errors[0].place.as_str()),
            ("Bad_kind_", "a_place_with_lines")
        );
        assert_eq!((report.app.as_str(), report.platform.as_str()), ("desktop", "native"));
        assert!(["windows", "macos", "linux", "other"].contains(&report.os.as_str()));
        assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn source_paths_never_say_where_they_were_built() {
        assert_eq!(source_path("src/ui/chat.rs"), "desktop/src/ui/chat.rs");
        assert_eq!(source_path("/home/someone/fuwa/desktop/src/ui/chat.rs"), "desktop/src/ui/chat.rs");
        assert_eq!(source_path(r"C:\Users\someone\fuwa\desktop\src\main.rs"), "desktop/src/main.rs");
        assert_eq!(
            source_path("/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.0/src/sync/mpsc.rs"),
            "tokio-1.47.0/src/sync/mpsc.rs"
        );
        assert_eq!(source_path("/rustc/abc123/library/core/src/option.rs"), "core/src/option.rs");
        assert_eq!(source_path("main.rs"), "main.rs");
    }

    #[test]
    fn rpc_methods_are_named_as_the_protocol_names_them() {
        assert_eq!(&*rpc_metric("MessageService", "send_message"), "rpc:MessageService/SendMessage");
        assert_eq!(&*rpc_metric("NodeService", "get_node"), "rpc:NodeService/GetNode");
    }

    #[test]
    fn a_report_that_failed_comes_back() {
        let mut held = Collector::default();
        held.error("panic", "desktop/src/ui/chat.rs:120", 1);
        held.timing("catch_up", 300);
        held.used("message.send", 2);
        let report = std::mem::take(&mut held);
        // More happens while it's on its way.
        held.used("message.send", 1);
        held.timing("catch_up", 40);
        held.merge(report);
        assert_eq!(held.usage["message.send"], 3);
        assert_eq!(held.errors[&("panic".into(), "desktop/src/ui/chat.rs:120".into())], 1);
        let timing = held.timings["catch_up"];
        assert_eq!((timing.count(), timing.sum_ms), (2, 340));

        // Coming back into a full report keeps to the limits.
        let mut full = Collector::default();
        for n in 0..MAX_ENTRIES {
            full.used(&format!("f{n}"), 1);
        }
        full.merge(held);
        assert_eq!(full.usage.len(), MAX_ENTRIES);
        assert!(!full.usage.contains_key("message.send"));
    }

    #[test]
    fn crashes_are_read_back_from_their_file() {
        let mut held = Collector::default();
        held.error("panic", "desktop/src/ui/chat.rs:120", 2);
        held.error("rpc_internal", "NodeService/GetNode", 1);
        let lines = held.crash_lines();
        assert_eq!(lines, "panic\tdesktop/src/ui/chat.rs:120\t2\n");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("crashes.txt");
        std::fs::write(&file, &lines).unwrap();
        note_crash(&file, "desktop/src/core/sync.rs:9");
        let mut next = Collector::default();
        next.read_crash_lines(&std::fs::read_to_string(&file).unwrap());
        next.read_crash_lines("garbage\nother\tline\n");
        assert_eq!(next.pending().errors, 3);
        assert_eq!(next.errors[&("panic".into(), "desktop/src/core/sync.rs:9".into())], 1);
    }
}
