//! GIPHY (developers.giphy.com): search and trending, with an API key in
//! the query. Its terms ask apps to show "Powered by GIPHY" where results are.

use serde_json::Value;

use super::{Ask, Found, Page, Rendition, Setup};

const BASE: &str = "https://api.giphy.com";

/// What to ask GIPHY.
pub fn address(base: Option<&str>, setup: &Setup, ask: Ask<'_>) -> reqwest::Url {
    let path = if ask.query.is_empty() { "/v1/gifs/trending" } else { "/v1/gifs/search" };
    let mut url = reqwest::Url::parse(&format!("{}{path}", base.unwrap_or(BASE))).expect("the address is valid");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("api_key", &setup.api_key);
        if !ask.query.is_empty() {
            q.append_pair("q", ask.query);
        }
        q.append_pair("limit", &ask.limit.to_string());
        q.append_pair("offset", &ask.cursor.to_string());
        q.append_pair("rating", setup.rating());
        q.append_pair("bundle", "messaging_non_clips");
    }
    url
}

/// A number GIPHY may send as text.
pub(super) fn number(value: &Value) -> u64 {
    match value {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn rendition(value: &Value) -> Option<Rendition> {
    let url = value.get("url")?.as_str()?;
    if url.is_empty() {
        return None;
    }
    Some(Rendition {
        url: url.to_string(),
        width: number(value.get("width").unwrap_or(&Value::Null)) as u32,
        height: number(value.get("height").unwrap_or(&Value::Null)) as u32,
        size: number(value.get("size").unwrap_or(&Value::Null)),
    })
}

/// GIPHY's answer as a page; `None` when it doesn't read.
pub fn read(body: &[u8], ask: Ask<'_>) -> Option<Page> {
    let answer: Value = serde_json::from_slice(body).ok()?;
    let mut found = Vec::new();
    for item in answer.get("data")?.as_array()? {
        let Some(id) = item.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else { continue };
        let Some(images) = item.get("images") else { continue };
        let Some(preview) = images.get("fixed_width").and_then(rendition) else { continue };
        let mut full: Vec<Rendition> = Vec::new();
        for name in ["original", "downsized_medium", "downsized", "fixed_width"] {
            if let Some(r) = images.get(name).and_then(rendition)
                && !full.iter().any(|kept| kept.url == r.url)
            {
                full.push(r);
            }
        }
        found.push(Found {
            id: id.to_string(),
            title: item.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
            still: images.get("fixed_width_still").and_then(rendition),
            preview,
            full,
        });
        if found.len() >= ask.limit as usize {
            break;
        }
    }
    let pagination = answer.get("pagination");
    let field = |name| pagination.and_then(|p| p.get(name)).map(number).unwrap_or(0);
    let (offset, count, total) = (field("offset"), field("count"), field("total_count"));
    let next = (count > 0 && offset + count < total).then(|| (offset + count).to_string());
    Some(Page { found, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gifs::Kind;

    #[test]
    fn asks_without_saying_who() {
        let setup = Setup { provider: Kind::Giphy, api_key: "key".into(), ..Default::default() };
        let url = address(None, &setup, Ask { query: "happy cat", cursor: 24, limit: 24 });
        assert_eq!(url.host_str(), Some("api.giphy.com"));
        assert_eq!(url.path(), "/v1/gifs/search");
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert!(pairs.contains(&("q".into(), "happy cat".into())));
        assert!(pairs.contains(&("offset".into(), "24".into())));
        assert!(pairs.contains(&("rating".into(), "pg-13".into())));
        // Nothing that would tell GIPHY who's searching.
        assert!(!pairs.iter().any(|(name, _)| name == "random_id" || name == "user_id"));
        let trending = address(Some("http://127.0.0.1:9"), &setup, Ask { query: "", cursor: 0, limit: 6 });
        assert_eq!(trending.path(), "/v1/gifs/trending");
        assert!(!trending.query_pairs().any(|(name, _)| name == "q"));
    }

    #[test]
    fn reads_an_answer() {
        let body = serde_json::json!({
            "data": [{
                "id": "abc",
                "title": "Happy Cat GIF",
                "images": {
                    "original": {"url": "https://media.giphy.com/media/abc/giphy.gif", "width": "480", "height": "270", "size": "2500000"},
                    "downsized": {"url": "https://media.giphy.com/media/abc/giphy-downsized.gif", "width": "320", "height": "180", "size": "900000"},
                    "fixed_width": {"url": "https://media.giphy.com/media/abc/200w.gif", "width": "200", "height": "113", "size": "300000"},
                    "fixed_width_still": {"url": "https://media.giphy.com/media/abc/200w_s.gif", "width": "200", "height": "113"}
                }
            }, {"id": "", "images": {}}],
            "pagination": {"total_count": 100, "count": 1, "offset": 0}
        });
        let page = read(body.to_string().as_bytes(), Ask { query: "cat", cursor: 0, limit: 24 }).unwrap();
        assert_eq!(page.found.len(), 1);
        let found = &page.found[0];
        assert_eq!(found.preview.width, 200);
        assert_eq!(found.full.len(), 3);
        assert_eq!(found.full[0].size, 2_500_000);
        assert!(found.still.is_some());
        assert_eq!(page.next.as_deref(), Some("1"));
        assert!(read(b"not json", Ask { query: "", cursor: 0, limit: 1 }).is_none());
    }
}
