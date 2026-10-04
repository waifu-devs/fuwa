//! `/healthz/parts`: which parts of an instance are up, for a status page.
//!
//! The directory answers it (behind the cluster key, like every call to an
//! internal part) with itself, every shard it knows of and whether it's
//! following the directory, and whether its calls part answers. A gateway
//! answers it for everyone by asking the directory; a single process says
//! it's up. Nothing in it names an address: parts are a kind and, for
//! shards, their id. Answers are reused for a few seconds, so asking often
//! costs the parts nothing.
//!
//! Parts that hold servers or streams say how busy they are (`load`):
//! streams open, writes running or waiting, writes a minute. With the admin
//! token (or, between parts, the cluster key) the answer also names the
//! busiest servers, by id and those counts only, never what's in them, so an
//! admin can see a noisy server and move it to a quieter shard.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::response::{IntoResponse, Response};
use http::{HeaderMap, StatusCode, header};
use serde_json::{Value, json};

use crate::app::{App, Link};

/// How long an answer is reused.
const FRESH: Duration = Duration::from_secs(10);
/// How long a part has to answer.
const TIMEOUT: Duration = Duration::from_secs(3);

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        // Only ever the instance's own parts, over its private network.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .expect("the HTTP client builds")
});

/// The last answer, and when it was made. While one caller makes a new
/// answer, the others wait for it rather than each asking the parts.
#[derive(Default)]
pub struct Cached(Mutex<Option<(Instant, Value)>>, tokio::sync::Mutex<()>);

impl Cached {
    /// The fresh answer, or a new one from `make`, made once however many ask at a time.
    async fn get_or_make<F: std::future::Future<Output = Value>>(&self, make: impl FnOnce() -> F) -> Value {
        if let Some(value) = self.fresh() {
            return value;
        }
        let _making = self.1.lock().await;
        // Someone else may have made it while this waited.
        if let Some(value) = self.fresh() {
            return value;
        }
        let value = make().await;
        self.keep(&value);
        value
    }

    fn fresh(&self) -> Option<Value> {
        let held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.as_ref().filter(|(at, _)| at.elapsed() < FRESH).map(|(_, value)| value.clone())
    }

    fn keep(&self, value: &Value) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), value.clone()));
    }
}

pub fn json_response(value: &Value) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], value.to_string())
        .into_response()
}

/// Whether a part answers its `/healthz`.
async fn answers(url: &str) -> bool {
    match CLIENT.get(format!("{url}/healthz")).send().await {
        Ok(response) => response.status() == StatusCode::OK,
        Err(_) => false,
    }
}

/// Busiest servers named per part, and in all.
const BUSIEST: usize = 5;

/// When the last look was, and each server's writes then.
type Writes = (Option<Instant>, HashMap<String, u64>);

/// How busy this process is, with its busiest servers under `busiest`.
pub fn load(app: &App) -> Value {
    // Each server's writes at the last look, to make a rate of.
    static LAST: LazyLock<Mutex<Writes>> = LazyLock::new(Default::default);
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    let minutes = last.0.map(|at| now.duration_since(at).as_secs_f64() / 60.0).filter(|m| *m > 0.0);
    let (mut waiting, mut per_minute) = (0usize, 0.0f64);
    let mut servers = Vec::new();
    let mut seen = HashMap::new();
    for sdb in app.servers.all() {
        let (writes, queued) = sdb.load();
        let rate = match (minutes, last.1.get(&sdb.id)) {
            (Some(minutes), Some(before)) => writes.saturating_sub(*before) as f64 / minutes,
            _ => 0.0,
        };
        waiting += queued;
        per_minute += rate;
        let followers = app.hub.followers(&sdb.id);
        if rate > 0.0 || queued > 0 || followers > 0 {
            servers.push((sdb.id.clone(), rate, queued, followers));
        }
        seen.insert(sdb.id.clone(), writes);
    }
    *last = (Some(now), seen);
    drop(last);
    servers.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(b.3.cmp(&a.3)));
    let busiest: Vec<Value> = servers
        .into_iter()
        .take(BUSIEST)
        .map(|(id, rate, queued, followers)| {
            json!({ "server_id": id, "writes_per_minute": rate.round() as u64, "writes_waiting": queued, "followers": followers })
        })
        .collect();
    json!({
        "streams": app.streams.count(),
        "writes_waiting": waiting,
        "writes_per_minute": per_minute.round() as u64,
        "busiest": busiest,
    })
}

/// Whether a request carries the instance's admin token.
pub fn is_admin(headers: &HeaderMap, admin_token: Option<&str>) -> bool {
    let given = crate::auth::bearer_header(headers);
    match (given, admin_token) {
        (Some(given), Some(token)) => crate::auth::constant_time_eq(given.as_bytes(), token.as_bytes()),
        _ => false,
    }
}

/// An answer for anyone: the busiest servers left out.
pub fn without_busiest(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("busiest");
    }
    if let Some(parts) = value.get_mut("parts").and_then(Value::as_array_mut) {
        for part in parts {
            if let Some(load) = part.get_mut("load").and_then(Value::as_object_mut) {
                load.remove("busiest");
            }
        }
    }
    value
}

/// The busiest of every part's busiest, each with the shard it's on.
fn busiest_of(parts: &[Value]) -> Vec<Value> {
    let mut all: Vec<Value> = parts
        .iter()
        .flat_map(|part| {
            let shard = part.get("id").cloned().unwrap_or(Value::Null);
            let busiest = part.pointer("/load/busiest").and_then(Value::as_array).cloned().unwrap_or_default();
            busiest.into_iter().map(move |mut server| {
                if let Some(object) = server.as_object_mut()
                    && !shard.is_null()
                {
                    object.insert("shard".into(), shard.clone());
                }
                server
            })
        })
        .collect();
    let rate = |v: &Value| v.get("writes_per_minute").and_then(Value::as_u64).unwrap_or(0);
    all.sort_by_key(|v| std::cmp::Reverse(rate(v)));
    all.truncate(BUSIEST);
    all
}

/// A shard's load, as its directory asks for it.
async fn shard_load(url: &str, key: &str) -> Option<Value> {
    let asked = CLIENT.get(format!("{url}/healthz/load")).header(super::KEY_HEADER, key).send().await.ok()?;
    if asked.status() != StatusCode::OK {
        return None;
    }
    asked.json::<Value>().await.ok().filter(Value::is_object)
}

/// What this process knows of the instance's parts. A directory's answer
/// (only parts with the cluster key reach it) and an admin's name the
/// busiest servers.
pub async fn parts(app: &Arc<App>, headers: &HeaderMap) -> Response {
    static CACHE: LazyLock<Cached> = LazyLock::new(Cached::default);
    let shards = match &app.link {
        Link::Alone => None,
        Link::Directory(shards) => Some(shards),
        Link::Shard(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let value = CACHE
        .get_or_make(|| async {
            let mut parts = Vec::new();
            let Some(shards) = shards else {
                let load = load(app);
                let busiest = load["busiest"].clone();
                return json!({ "parts": [{ "part": "server", "up": true, "load": load }], "busiest": busiest });
            };
            parts.push(json!({ "part": "directory", "up": true }));
            let key = app.config.cluster.key.as_deref().unwrap_or_default();
            let urls = shards.urls();
            let loads = futures::future::join_all(shards.all().into_iter().map(|(id, client)| {
                let url = urls.get(&id).filter(|_| client.is_some()).cloned();
                async move {
                    let load = match url {
                        Some(url) => shard_load(&url, key).await,
                        None => None,
                    };
                    (id, client.is_some(), load)
                }
            }))
            .await;
            for (id, up, load) in loads {
                let mut part = json!({ "part": "shard", "id": id, "up": up });
                if let Some(load) = load {
                    part["load"] = load;
                }
                parts.push(part);
            }
            let urls = &app.config.media_urls;
            if !urls.is_empty() {
                // One entry for calls: up while every calls part answers.
                let up = futures::future::join_all(urls.iter().map(|url| answers(url))).await.into_iter().all(|up| up);
                parts.push(json!({ "part": "media", "up": up }));
            }
            let busiest = busiest_of(&parts);
            json!({ "parts": parts, "busiest": busiest })
        })
        .await;
    let admin = shards.is_some() || is_admin(headers, app.config.admin_token.as_deref());
    json_response(&if admin { value } else { without_busiest(value) })
}

/// A gateway's answer: the directory's, or the directory down when it can't
/// be had, with this gateway's own streams. The busiest servers only for an admin.
pub async fn from_directory(cache: &Cached, directory_url: &str, key: &str, streams: usize, admin: bool) -> Response {
    let mut value = cache
        .get_or_make(|| async {
            let asked =
                CLIENT.get(format!("{directory_url}/healthz/parts")).header(super::KEY_HEADER, key).send().await;
            match asked {
                Ok(response) if response.status() == StatusCode::OK => response.json::<Value>().await.ok(),
                _ => None,
            }
            .filter(|value| value.get("parts").is_some_and(Value::is_array))
            .unwrap_or_else(|| json!({ "parts": [{ "part": "directory", "up": false }] }))
        })
        .await;
    // The gateway asked is one of several, so it says so.
    if let Some(parts) = value.get_mut("parts").and_then(Value::as_array_mut) {
        parts.push(json!({ "part": "gateway", "this": true, "up": true, "load": { "streams": streams } }));
    }
    json_response(&if admin { value } else { without_busiest(value) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_reused_while_fresh() {
        let cache = Cached::default();
        assert!(cache.fresh().is_none());
        cache.keep(&json!({ "parts": [] }));
        assert_eq!(cache.fresh(), Some(json!({ "parts": [] })));
        *cache.0.lock().unwrap() = Some((Instant::now() - FRESH, json!({})));
        assert!(cache.fresh().is_none());
    }

    #[test]
    fn busiest_only_for_admins() {
        let parts = vec![
            json!({ "part": "shard", "id": "shard-a", "up": true, "load": { "streams": 3, "busiest": [
                { "server_id": "s1", "writes_per_minute": 40 }, { "server_id": "s2", "writes_per_minute": 5 } ] } }),
            json!({ "part": "shard", "id": "shard-b", "up": true, "load": { "streams": 1, "busiest": [
                { "server_id": "s3", "writes_per_minute": 90 } ] } }),
        ];
        let busiest = busiest_of(&parts);
        let named: Vec<_> = busiest.iter().map(|s| (s["server_id"].clone(), s["shard"].clone())).collect();
        assert_eq!(
            named,
            [(json!("s3"), json!("shard-b")), (json!("s1"), json!("shard-a")), (json!("s2"), json!("shard-a"))]
        );
        let public = without_busiest(json!({ "parts": parts, "busiest": busiest }));
        assert!(!public.to_string().contains("busiest"), "{public}");
        assert_eq!(public["parts"][0]["load"]["streams"], 3);
    }

    #[test]
    fn admin_token_checked() {
        let mut headers = HeaderMap::new();
        assert!(!is_admin(&headers, Some("secret")));
        headers.insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(!is_admin(&headers, Some("secret")));
        assert!(!is_admin(&headers, None));
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(is_admin(&headers, Some("secret")));
    }

    #[tokio::test]
    async fn callers_at_once_make_one_answer() {
        let cache = Arc::new(Cached::default());
        let made = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = (0..20).map(|_| {
            let (cache, made) = (cache.clone(), made.clone());
            tokio::spawn(async move {
                cache
                    .get_or_make(|| async {
                        made.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        json!({ "parts": [] })
                    })
                    .await
            })
        });
        for call in futures::future::join_all(calls).await {
            assert_eq!(call.unwrap(), json!({ "parts": [] }));
        }
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
