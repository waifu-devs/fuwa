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
    let shards = match &app.link {
        Link::Alone => None,
        Link::Directory(shards) => Some(shards),
        Link::Shard(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let value = CACHE
        .get_or_make(|| async {
            let mut parts = Vec::new();
            let Some(shards) = shards else {
                return json!({ "parts": [{ "part": "server", "up": true }] });
            };
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
            json!({ "parts": parts })
        })
        .await;
    json_response(&value)
}

/// A gateway's answer: the directory's, or the directory down when it can't be had.
pub async fn from_directory(cache: &Cached, directory_url: &str, key: &str) -> Response {
    let value = cache
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
