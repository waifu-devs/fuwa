//! `/healthz/parts`: which parts of an instance are up, for a status page.
//!
//! The directory answers it (behind the cluster key, like every call to an
//! internal part) with itself, every shard it knows of and whether it's
//! following the directory, and whether its calls part answers. A gateway
//! answers it for everyone by asking the directory; a single process says
//! it's up. Nothing in it names an address: parts are a kind and, for
//! shards, their id. Answers are reused for a few seconds, so asking often
//! costs the parts nothing.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
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

/// The last answer, and when it was made.
#[derive(Default)]
pub struct Cached(Mutex<Option<(Instant, Value)>>);

impl Cached {
    fn fresh(&self) -> Option<Value> {
        let held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.as_ref().filter(|(at, _)| at.elapsed() < FRESH).map(|(_, value)| value.clone())
    }

    fn keep(&self, value: &Value) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), value.clone()));
    }
}

fn json_response(value: &Value) -> Response {
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

/// What this process knows of the instance's parts.
pub async fn parts(app: &Arc<App>) -> Response {
    static CACHE: LazyLock<Cached> = LazyLock::new(Cached::default);
    if let Some(value) = CACHE.fresh() {
        return json_response(&value);
    }
    let mut parts = Vec::new();
    match &app.link {
        Link::Alone => parts.push(json!({ "part": "server", "up": true })),
        Link::Directory(shards) => {
            parts.push(json!({ "part": "directory", "up": true }));
            for (id, client) in shards.all() {
                parts.push(json!({ "part": "shard", "id": id, "up": client.is_some() }));
            }
            let urls = &app.config.media_urls;
            if !urls.is_empty() {
                // One entry for calls: up while every calls part answers.
                let up = futures::future::join_all(urls.iter().map(|url| answers(url))).await.into_iter().all(|up| up);
                parts.push(json!({ "part": "media", "up": up }));
            }
        }
        Link::Shard(_) => return StatusCode::NOT_FOUND.into_response(),
    }
    let value = json!({ "parts": parts });
    CACHE.keep(&value);
    json_response(&value)
}

/// A gateway's answer: the directory's, or the directory down when it can't be had.
pub async fn from_directory(cache: &Cached, directory_url: &str, key: &str) -> Response {
    if let Some(value) = cache.fresh() {
        return json_response(&value);
    }
    let asked = CLIENT.get(format!("{directory_url}/healthz/parts")).header(super::KEY_HEADER, key).send().await;
    let value = match asked {
        Ok(response) if response.status() == StatusCode::OK => response.json::<Value>().await.ok(),
        _ => None,
    }
    .filter(|value| value.get("parts").is_some_and(Value::is_array))
    .unwrap_or_else(|| json!({ "parts": [{ "part": "directory", "up": false }] }));
    cache.keep(&value);
    json_response(&value)
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
}
