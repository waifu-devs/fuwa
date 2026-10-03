//! Anonymous health reports: what went wrong and what was slow, as counts. The
//! server counts its own panics, failed requests and request times here, adds
//! the reports its apps send (`NodeService.SendReport`), and once an hour sends
//! the total to waifu.dev so we can find bugs and slow paths. Like the usage
//! signal (`telemetry.rs`) it's off with FUWA_TELEMETRY=off or the instance
//! setting, and it never holds message content, names, ids of people or
//! servers, file names or addresses: only kinds of failure, places in the
//! code, durations in buckets and feature counts. README.md lists every field.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::app::HasSettings;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms};
use crate::pb;

/// Bumped only for breaking changes; new fields can join v1.
pub const SCHEMA: &str = "fuwa.report.v1";

/// Upper bounds of the timing buckets, in milliseconds; one more bucket holds
/// anything longer. Apps use the same ones (see `ReportTiming`).
pub const BOUNDS_MS: [u64; 12] = [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 30000];
const BUCKETS: usize = BOUNDS_MS.len() + 1;

const INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long the last report may take on the way out.
const LAST_SEND: Duration = Duration::from_secs(5);

/// Distinct entries one report holds of each kind; more are counted in `dropped`.
const MAX_ENTRIES: usize = 512;
/// Entries of each kind one app report may carry.
const MAX_APP_ENTRIES: usize = 64;
/// One app report counts at most this much of any one thing.
const MAX_APP_COUNT: u32 = 10_000;
/// How often one account may send an app report.
const APP_REPORT_EVERY_MS: i64 = 60 * 1000;

/// Which app, which build, on what.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Origin {
    app: String,
    version: String,
    platform: String,
    os: String,
}

impl Origin {
    fn server() -> Self {
        Self {
            app: "server".into(),
            version: crate::VERSION.into(),
            platform: "server".into(),
            os: std::env::consts::OS.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Timing {
    buckets: [u64; BUCKETS],
    sum_ms: u64,
}

impl Timing {
    fn add(&mut self, ms: u64) {
        let bucket = BOUNDS_MS.iter().position(|&bound| ms <= bound).unwrap_or(BOUNDS_MS.len());
        self.buckets[bucket] += 1;
        self.sum_ms = self.sum_ms.saturating_add(ms);
    }
}

/// Everything counted since the last report.
#[derive(Debug, Default)]
struct Counts {
    since_ms: i64,
    errors: HashMap<(Origin, String, String), u64>,
    timings: HashMap<(Origin, String), Timing>,
    usage: HashMap<(Origin, String), u64>,
    dropped: u64,
}

impl Counts {
    fn is_empty(&self) -> bool {
        self.errors.is_empty() && self.timings.is_empty() && self.usage.is_empty() && self.dropped == 0
    }

    fn error(&mut self, origin: &Origin, kind: &str, place: &str, count: u64) {
        let key = (origin.clone(), kind.to_string(), place.to_string());
        if let Some(total) = self.errors.get_mut(&key) {
            *total += count;
        } else if self.errors.len() < MAX_ENTRIES {
            self.errors.insert(key, count);
        } else {
            self.dropped += 1;
        }
    }

    fn timing(&mut self, origin: &Origin, metric: &str, f: impl FnOnce(&mut Timing)) {
        let key = (origin.clone(), metric.to_string());
        if let Some(timing) = self.timings.get_mut(&key) {
            f(timing);
        } else if self.timings.len() < MAX_ENTRIES {
            f(self.timings.entry(key).or_default());
        } else {
            self.dropped += 1;
        }
    }

    fn used(&mut self, origin: &Origin, feature: &str, count: u64) {
        let key = (origin.clone(), feature.to_string());
        if let Some(total) = self.usage.get_mut(&key) {
            *total += count;
        } else if self.usage.len() < MAX_ENTRIES {
            self.usage.insert(key, count);
        } else {
            self.dropped += 1;
        }
    }
}

struct Collector {
    server: Origin,
    counts: Mutex<Counts>,
    /// When each account last sent an app report, for the last minute only.
    last_app_report: Mutex<HashMap<String, i64>>,
}

static COLLECTOR: LazyLock<Collector> = LazyLock::new(|| Collector {
    server: Origin::server(),
    counts: Mutex::new(Counts { since_ms: now_ms(), ..Counts::default() }),
    last_app_report: Mutex::new(HashMap::new()),
});

fn counts() -> std::sync::MutexGuard<'static, Counts> {
    COLLECTOR.counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

tokio::task_local! {
    /// The gRPC method being answered, such as "MessageService/SendMessage".
    static RPC: Arc<str>;
}

/// Counts a failure in the server itself. `place` defaults to the gRPC method
/// being answered.
pub fn server_error(kind: &str, place: Option<&str>) {
    let place = match place {
        Some(place) => clean(place, 120),
        None => RPC.try_with(|method| method.to_string()).unwrap_or_else(|_| "background".into()),
    };
    counts().error(&COLLECTOR.server, kind, &place, 1);
}

/// Counts how long something in the server took.
pub fn server_timing(metric: &str, took: Duration) {
    let ms = took.as_millis().min(u64::MAX as u128) as u64;
    counts().timing(&COLLECTOR.server, metric, |timing| timing.add(ms));
}

/// The gRPC method a path names, for fuwa's own services only.
fn rpc_method(path: &str) -> Option<&str> {
    path.strip_prefix("/fuwa.v1.").filter(|method| method.contains('/'))
}

/// Middleware timing each call to fuwa's services (until the response starts:
/// a stream counts the time to its first answer) and naming the method for
/// failures counted while it runs.
pub async fn time_calls(request: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let Some(method) = rpc_method(request.uri().path()).map(Arc::<str>::from) else {
        return next.run(request).await;
    };
    let started = Instant::now();
    let response = RPC.scope(method.clone(), next.run(request)).await;
    server_timing(&format!("rpc:{method}"), started.elapsed());
    response
}

/// Counts panics by where they happened, then lets the usual hook print them.
pub fn count_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let place = info.location().map(|location| format!("{}:{}", source_path(location.file()), location.line()));
        // A panic while the counts are locked must not panic again here.
        if let Ok(mut counts) = COLLECTOR.counts.try_lock() {
            counts.error(&COLLECTOR.server, "panic", place.as_deref().unwrap_or("unknown"), 1);
        }
        previous(info);
    }));
}

/// A source file as a path inside its crate, such as "server/src/app.rs" or
/// "tokio-1.47.0/src/sync/mpsc.rs": never where it was built.
fn source_path(file: &str) -> String {
    let file = file.replace('\\', "/");
    let trimmed = match file.rfind("/src/") {
        Some(at) => {
            let start = file[..at].rfind('/').map(|slash| slash + 1).unwrap_or(0);
            &file[start..]
        }
        None => file.rsplit('/').next().unwrap_or(&file),
    };
    clean(trimmed, 110)
}

/// A label made safe: allowed characters only, and short.
fn clean(value: &str, max: usize) -> String {
    value.chars().take(max).map(|c| if allowed(c) { c } else { '_' }).collect()
}

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '#' | '@' | '-')
}

/// Checks a label an app sent.
fn label<'a>(what: &str, value: &'a str, max: usize) -> Result<&'a str> {
    if value.is_empty() || value.len() > max || !value.chars().all(allowed) {
        return Err(Error::invalid(format!(
            "{what} must be 1 to {max} letters, digits or _.:/#@- (got {} characters)",
            value.chars().count()
        )));
    }
    Ok(value)
}

/// Adds an app's report to the counts, unless the instance's telemetry is
/// off. `account_id` only limits how often one account may send; it's
/// forgotten within a minute.
pub fn add_app_report(account_id: &str, telemetry: bool, report: &pb::AppReport) -> Result<()> {
    let origin = Origin {
        app: match report.app.as_str() {
            "web" | "desktop" => report.app.clone(),
            _ => return Err(Error::invalid("app must be web or desktop")),
        },
        version: label("version", &report.version, 32)?.to_string(),
        platform: label("platform", &report.platform, 16)?.to_ascii_lowercase(),
        os: label("os", &report.os, 16)?.to_ascii_lowercase(),
    };
    if report.errors.len() > MAX_APP_ENTRIES
        || report.timings.len() > MAX_APP_ENTRIES
        || report.usage.len() > MAX_APP_ENTRIES
    {
        return Err(Error::invalid(format!("a report carries at most {MAX_APP_ENTRIES} entries of each kind")));
    }
    for error in &report.errors {
        label("kind", &error.kind, 48)?;
        label("place", &error.place, 120)?;
    }
    for timing in &report.timings {
        label("metric", &timing.metric, 96)?;
        if timing.buckets.len() != BUCKETS {
            return Err(Error::invalid(format!("a timing has {BUCKETS} buckets")));
        }
    }
    for usage in &report.usage {
        label("feature", &usage.feature, 96)?;
    }

    {
        let mut last = COLLECTOR.last_app_report.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = now_ms();
        last.retain(|_, at| now - *at < APP_REPORT_EVERY_MS);
        if last.contains_key(account_id) {
            return Err(Error::ResourceExhausted("one report a minute; keep it and send it with the next".into()));
        }
        last.insert(account_id.to_string(), now);
    }
    if !telemetry {
        return Ok(());
    }

    let count = |n: u32| u64::from(n.min(MAX_APP_COUNT));
    let mut counts = counts();
    for error in &report.errors {
        if error.count > 0 {
            counts.error(&origin, &error.kind, &error.place, count(error.count));
        }
    }
    for timing in &report.timings {
        if timing.buckets.iter().all(|&n| n == 0) {
            continue;
        }
        let n: u64 = timing.buckets.iter().map(|&n| count(n)).sum();
        // No more than 30 seconds per duration on average, so one app can't skew the sums.
        let sum = timing.sum_ms.min(n.saturating_mul(BOUNDS_MS[BOUNDS_MS.len() - 1]));
        counts.timing(&origin, &timing.metric, |total| {
            for (into, &from) in total.buckets.iter_mut().zip(&timing.buckets) {
                *into += count(from);
            }
            total.sum_ms = total.sum_ms.saturating_add(sum);
        });
    }
    for usage in &report.usage {
        if usage.count > 0 {
            counts.used(&origin, &usage.feature, count(usage.count));
        }
    }
    Ok(())
}

/// What leaves the instance, as JSON.
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    /// Random per report, so one delivered twice counts once.
    pub report_id: String,
    /// The instance's install id (the usage signal's), on the parts that keep
    /// accounts; shards and gateways send none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub hosting: &'static str,
    /// Which part of the instance sent it: all, directory, shard or gateway.
    pub part: &'static str,
    /// The period counted, in Unix milliseconds.
    pub since: i64,
    pub sent_at: i64,
    pub bounds_ms: &'static [u64],
    pub errors: Vec<ErrorEntry>,
    pub timings: Vec<TimingEntry>,
    pub usage: Vec<UsageEntry>,
    /// Entries left out because the report was full.
    pub dropped: u64,
}

#[derive(Debug, Serialize)]
pub struct ErrorEntry {
    pub app: String,
    pub version: String,
    pub platform: String,
    pub os: String,
    pub kind: String,
    pub place: String,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct TimingEntry {
    pub app: String,
    pub version: String,
    pub platform: String,
    pub os: String,
    pub metric: String,
    pub buckets: Vec<u64>,
    pub count: u64,
    pub sum_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct UsageEntry {
    pub app: String,
    pub version: String,
    pub platform: String,
    pub os: String,
    pub feature: String,
    pub count: u64,
}

/// Takes everything counted so far, leaving the counts empty. None when there
/// was nothing to count.
fn take(install_id: Option<String>, hosted: bool, part: &'static str) -> Option<Report> {
    let now = now_ms();
    let counts = std::mem::replace(&mut *counts(), Counts { since_ms: now, ..Counts::default() });
    if counts.is_empty() {
        return None;
    }
    let mut report = Report {
        schema: SCHEMA,
        report_id: new_id(),
        install_id,
        hosting: if hosted { "hosted" } else { "self_hosted" },
        part,
        since: counts.since_ms,
        sent_at: now,
        bounds_ms: &BOUNDS_MS,
        errors: counts
            .errors
            .into_iter()
            .map(|((o, kind, place), count)| ErrorEntry {
                app: o.app,
                version: o.version,
                platform: o.platform,
                os: o.os,
                kind,
                place,
                count,
            })
            .collect(),
        timings: counts
            .timings
            .into_iter()
            .map(|((o, metric), t)| TimingEntry {
                app: o.app,
                version: o.version,
                platform: o.platform,
                os: o.os,
                metric,
                buckets: t.buckets.to_vec(),
                count: t.buckets.iter().sum(),
                sum_ms: t.sum_ms,
            })
            .collect(),
        usage: counts
            .usage
            .into_iter()
            .map(|((o, feature), count)| UsageEntry {
                app: o.app,
                version: o.version,
                platform: o.platform,
                os: o.os,
                feature,
                count,
            })
            .collect(),
        dropped: counts.dropped,
    };
    // Steady order, so the log reads the same way each time.
    report.errors.sort_by(|a, b| b.count.cmp(&a.count));
    report.timings.sort_by(|a, b| a.app.cmp(&b.app).then_with(|| a.metric.cmp(&b.metric)));
    report.usage.sort_by(|a, b| b.count.cmp(&a.count));
    Some(report)
}

/// Sends a report every hour while telemetry is on, and the last one on
/// shutdown. Await the handle before the process ends so that one gets out.
pub fn spawn(
    source: Arc<dyn HasSettings>,
    config: &Config,
    install_id: Option<String>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let url = config.telemetry.reports_url.clone();
    let hosted = config.telemetry.hosted;
    let part = config.cluster.role.as_str();
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent(format!("fuwa/{}", crate::VERSION))
            .timeout(Duration::from_secs(15))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(error = %err, "couldn't set up the health report; it stays off");
                return;
            }
        };
        let send = async |last: bool| {
            let report = take(install_id.clone(), hosted, part);
            // Off: what was counted is thrown away rather than kept for later.
            let Some(report) = report.filter(|_| source.settings().telemetry) else { return };
            let summary = format!(
                "{} errors, {} timings, {} usage counts",
                report.errors.len(),
                report.timings.len(),
                report.usage.len()
            );
            tracing::debug!(report = %serde_json::to_string(&report).unwrap_or_default(), "sending health report");
            let request = client.post(&url).json(&report);
            let request = if last { request.timeout(LAST_SEND) } else { request };
            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    tracing::info!("sent the anonymous health report ({summary})")
                }
                Ok(response) => tracing::debug!(status = %response.status(), "health report not taken; dropping it"),
                Err(err) => tracing::debug!(error = %err, "health report not sent; dropping it"),
            }
        };
        let mut every = tokio::time::interval_at(tokio::time::Instant::now() + INTERVAL, INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    send(true).await;
                    return;
                }
                _ = every.tick() => send(false).await,
            }
        }
    })
}

/// Waits a moment for the last report to get out.
pub async fn finish(handle: JoinHandle<()>) {
    let _ = tokio::time::timeout(LAST_SEND + Duration::from_secs(1), handle).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counts are one per process, so the tests that read them take turns.
    static TURNS: Mutex<()> = Mutex::new(());

    fn report() -> pb::AppReport {
        pb::AppReport {
            app: "web".into(),
            version: "0.4.2".into(),
            platform: "chromium".into(),
            os: "linux".into(),
            errors: vec![pb::ReportError { kind: "TypeError".into(), place: "settings/roles".into(), count: 2 }],
            timings: vec![pb::ReportTiming {
                metric: "startup".into(),
                buckets: vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
                sum_ms: 200,
            }],
            usage: vec![pb::ReportUsage { feature: "message.send".into(), count: 3 }],
        }
    }

    #[test]
    fn buckets_split_on_their_bounds() {
        let mut timing = Timing::default();
        for ms in [0, 5, 6, 30000, 30001, u64::MAX] {
            timing.add(ms);
        }
        assert_eq!(timing.buckets[0], 2);
        assert_eq!(timing.buckets[1], 1);
        assert_eq!(timing.buckets[11], 1);
        assert_eq!(timing.buckets[12], 2);
    }

    #[test]
    fn source_paths_never_say_where_they_were_built() {
        assert_eq!(source_path("/home/someone/fuwa/server/src/app.rs"), "server/src/app.rs");
        assert_eq!(
            source_path("/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.47.0/src/sync/mpsc.rs"),
            "tokio-1.47.0/src/sync/mpsc.rs"
        );
        assert_eq!(source_path(r"C:\Users\someone\fuwa\desktop\src\main.rs"), "desktop/src/main.rs");
        assert_eq!(source_path("main.rs"), "main.rs");
    }

    #[test]
    fn app_reports_are_checked_limited_and_counted() {
        let _turn = TURNS.lock().unwrap_or_else(|p| p.into_inner());
        let mut bad = report();
        bad.errors[0].place = "chat with juan@example.com".into();
        assert!(add_app_report("a-check", true, &bad).is_err());
        let mut bad = report();
        bad.app = "toaster".into();
        assert!(add_app_report("a-check", true, &bad).is_err());
        let mut bad = report();
        bad.timings[0].buckets.pop();
        assert!(add_app_report("a-check", true, &bad).is_err());

        add_app_report("a-counted", true, &report()).unwrap();
        // The same account again within the minute.
        assert!(matches!(add_app_report("a-counted", true, &report()), Err(Error::ResourceExhausted(_))));
        let counts = counts();
        let web =
            Origin { app: "web".into(), version: "0.4.2".into(), platform: "chromium".into(), os: "linux".into() };
        assert!(counts.errors.get(&(web.clone(), "TypeError".into(), "settings/roles".into())).copied() >= Some(2));
        assert!(counts.usage.get(&(web, "message.send".into())).copied() >= Some(3));
    }

    #[test]
    fn reports_are_taken_once() {
        let _turn = TURNS.lock().unwrap_or_else(|p| p.into_inner());
        server_error("test_kind", Some("reports::tests"));
        server_timing("test.metric", Duration::from_millis(42));
        let report = take(Some("01J0000000000000000000000".into()), false, "all").expect("something was counted");
        assert_eq!(report.schema, SCHEMA);
        assert!(report.errors.iter().any(|e| e.kind == "test_kind" && e.app == "server"));
        let timing = report.timings.iter().find(|t| t.metric == "test.metric").unwrap();
        assert_eq!((timing.count, timing.sum_ms, timing.buckets.len()), (1, 42, BUCKETS));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["bounds_ms"].as_array().unwrap().len(), BOUNDS_MS.len());
        assert!(!take(None, false, "all").is_some_and(|again| again.errors.iter().any(|e| e.kind == "test_kind")));
    }
}
