//! Posting through a webhook over plain HTTP:
//! `POST /webhooks/<server id>/<webhook id>/<token>` with a JSON body shaped
//! like Discord's, so tools made for Discord webhooks work unchanged. Served
//! where servers are kept (a single process, or the shard holding the server,
//! which the gateways pass these on to). See `proto/fuwa/v1/webhook.proto`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, RawQuery};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use http::{HeaderValue, StatusCode, header};
use serde::Deserialize;

use crate::api::{WebhookPost, execute_webhook, verify_webhook};
use crate::app::App;
use crate::error::{Error, MISROUTED};
use crate::id::now_ms;
use crate::pb;

/// The biggest body taken: a long message with ten full embeds fits easily.
const MAX_BODY: usize = 256 * 1024;
/// Messages one webhook may post a minute, as Discord allows.
const PER_MINUTE: usize = 30;
const MINUTE_MS: i64 = 60_000;

pub fn routes(app: Arc<App>) -> Router {
    let limiter = Arc::new(Limiter::default());
    Router::new()
        .route(
            "/webhooks/{server_id}/{webhook_id}/{token}",
            post(
                move |Path((server_id, webhook_id, token)): Path<(String, String, String)>,
                      RawQuery(query): RawQuery,
                      body: Bytes| {
                    let (app, limiter) = (app.clone(), limiter.clone());
                    async move {
                        let wait =
                            query.as_deref().is_some_and(|q| q.split('&').any(|p| p == "wait=true" || p == "wait=1"));
                        execute(&app, &limiter, &server_id, &webhook_id, &token, &body, wait).await
                    }
                },
            ),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

/// The last minute of posts, per webhook.
#[derive(Default)]
struct Limiter {
    posts: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl Limiter {
    /// Counts a post now, or says how many milliseconds until one is allowed.
    fn take(&self, webhook_id: &str, now: i64) -> Result<(), i64> {
        let mut posts = self.posts.lock().unwrap_or_else(|p| p.into_inner());
        // Forget webhooks that have been quiet a minute, so the map stays small.
        if posts.len() > 1024 {
            posts.retain(|_, times| times.back().is_some_and(|&t| now - t < MINUTE_MS));
        }
        let times = posts.entry(webhook_id.to_string()).or_default();
        while times.front().is_some_and(|&t| now - t >= MINUTE_MS) {
            times.pop_front();
        }
        if times.len() >= PER_MINUTE {
            return Err(times.front().map_or(MINUTE_MS, |&t| t + MINUTE_MS - now));
        }
        times.push_back(now);
        Ok(())
    }
}

/// A post as Discord's webhooks take it. Fields fuwa doesn't have are ignored.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Body {
    content: String,
    username: String,
    avatar_url: String,
    embeds: Vec<Embed>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Embed {
    title: String,
    description: String,
    url: String,
    color: i64,
    fields: Vec<Field>,
    thumbnail: Option<Picture>,
    image: Option<Picture>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Field {
    name: String,
    value: String,
    inline: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Picture {
    url: String,
}

impl From<Embed> for pb::Embed {
    fn from(e: Embed) -> Self {
        pb::Embed {
            title: e.title,
            description: e.description,
            url: e.url,
            color: (e.color & 0xFF_FFFF) as i32,
            fields: e
                .fields
                .into_iter()
                .map(|f| pb::EmbedField { name: f.name, value: f.value, inline: f.inline })
                .collect(),
            thumbnail_url: e.thumbnail.map(|p| p.url).unwrap_or_default(),
            image_url: e.image.map(|p| p.url).unwrap_or_default(),
        }
    }
}

fn answer(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({ "message": message });
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

async fn execute(
    app: &Arc<App>,
    limiter: &Limiter,
    server_id: &str,
    webhook_id: &str,
    token: &str,
    body: &[u8],
    wait: bool,
) -> Response {
    let body: Body = match serde_json::from_slice(body) {
        Ok(body) => body,
        Err(err) => return answer(StatusCode::BAD_REQUEST, &format!("the body isn't the JSON a webhook takes: {err}")),
    };
    // Only real posts count, so knowing a webhook's id (every member sees it)
    // isn't enough to hold it back.
    if let Err(err) = verify_webhook(app, server_id, webhook_id, token).await {
        return failed(err);
    }
    if let Err(ms) = limiter.take(webhook_id, now_ms()) {
        let seconds = (ms + 999) / 1000;
        let mut response = answer(StatusCode::TOO_MANY_REQUESTS, "this webhook is posting too fast; slow down");
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return response;
    }
    let post = WebhookPost {
        content: body.content,
        username: body.username,
        avatar_url: body.avatar_url,
        embeds: body.embeds.into_iter().map(Into::into).collect(),
    };
    match execute_webhook(app, server_id, webhook_id, token, post).await {
        Ok(message) if wait => {
            let json = serde_json::json!({
                "id": message.id,
                "channel_id": message.channel_id,
                "content": message.content,
                "timestamp": message.created_at.map(|t| crate::id::millis(&t)),
            });
            (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], json.to_string()).into_response()
        }
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => failed(err),
    }
}

/// What a post that failed answers.
fn failed(err: Error) -> Response {
    match err {
        Error::Misrouted => {
            let mut response = answer(StatusCode::SERVICE_UNAVAILABLE, "that server is on another shard");
            response.headers_mut().insert(MISROUTED, HeaderValue::from_static("1"));
            response
        }
        Error::Moving => {
            let mut response = answer(StatusCode::SERVICE_UNAVAILABLE, "that server is moving; try again in a moment");
            response.headers_mut().insert(crate::error::NOT_READY, HeaderValue::from_static("1"));
            response
        }
        Error::Limited(message, wait_ms) => {
            let mut response = answer(StatusCode::TOO_MANY_REQUESTS, &message);
            let seconds = (wait_ms.max(1) + 999) / 1000;
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(axum::http::header::RETRY_AFTER, value);
            }
            response
        }
        err => {
            let status = match &err {
                Error::InvalidArgument(_) => StatusCode::BAD_REQUEST,
                Error::NotFound(_) => StatusCode::NOT_FOUND,
                Error::PermissionDenied(_) => StatusCode::FORBIDDEN,
                Error::ResourceExhausted(_) => StatusCode::INSUFFICIENT_STORAGE,
                Error::Busy | Error::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
                _ => {
                    tracing::error!("a webhook post failed");
                    return answer(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong on the server");
                }
            };
            answer(status, &err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_each_webhook_to_thirty_a_minute() {
        let limiter = Limiter::default();
        for i in 0..PER_MINUTE as i64 {
            assert!(limiter.take("a", 1_000 + i).is_ok());
        }
        assert_eq!(limiter.take("a", 2_000), Err(MINUTE_MS - 1_000));
        assert!(limiter.take("b", 2_000).is_ok());
        assert!(limiter.take("a", 1_000 + MINUTE_MS).is_ok());
    }

    #[test]
    fn reads_discord_shaped_posts() {
        let body: Body = serde_json::from_str(
            r#"{"content":"hi","username":"CI","embeds":[{"title":"Build","color":16711680,
                "fields":[{"name":"a","value":"b","inline":true}],"thumbnail":{"url":"https://x/y.png"}}],
                "tts":false,"allowed_mentions":{"parse":[]}}"#,
        )
        .unwrap();
        assert_eq!(body.username, "CI");
        let embed: pb::Embed = body.embeds.into_iter().next().unwrap().into();
        assert_eq!(embed.color, 0xFF0000);
        assert_eq!(embed.thumbnail_url, "https://x/y.png");
        assert!(embed.fields[0].inline);
    }
}
