//! Klipy (klipy.com): search and trending, with the app key in the path.
//! Its answers come in sizes (hd, md, sm, xs), each in several formats; we
//! take the GIFs, and a still where it has one. Ads it mixes in are left
//! out, and no customer id is ever sent.

use serde_json::Value;

use super::giphy::number;
use super::{Ask, Found, Page, Rendition, Setup};

const BASE: &str = "https://api.klipy.com";

/// Klipy pages hold at least this many.
const MIN_PAGE: u32 = 8;

/// What to ask Klipy. Its pages are numbered from 1; ours from 0.
pub fn address(base: Option<&str>, setup: &Setup, ask: Ask<'_>) -> reqwest::Url {
    let path = if ask.query.is_empty() { "trending" } else { "search" };
    let mut url = reqwest::Url::parse(&format!("{}/api/v1/{}/gifs/{path}", base.unwrap_or(BASE), setup.api_key))
        .expect("the address is valid");
    {
        let mut q = url.query_pairs_mut();
        if !ask.query.is_empty() {
            q.append_pair("q", ask.query);
        }
        q.append_pair("page", &(ask.cursor + 1).to_string());
        q.append_pair("per_page", &ask.limit.clamp(MIN_PAGE, 50).to_string());
        q.append_pair("rating", setup.rating());
    }
    url
}

fn rendition(value: Option<&Value>) -> Option<Rendition> {
    let value = value?;
    let url = value.get("url")?.as_str()?;
    if url.is_empty() {
        return None;
    }
    Some(Rendition {
        url: url.to_string(),
        width: value.get("width").map(number).unwrap_or(0) as u32,
        height: value.get("height").map(number).unwrap_or(0) as u32,
        size: value.get("size").map(number).unwrap_or(0),
    })
}

/// Klipy's answer as a page; `None` when it doesn't read.
pub fn read(body: &[u8], ask: Ask<'_>) -> Option<Page> {
    let answer: Value = serde_json::from_slice(body).ok()?;
    let data = answer.get("data")?;
    let mut found = Vec::new();
    for item in data.get("data")?.as_array()? {
        if item.get("type").and_then(Value::as_str).is_some_and(|kind| kind != "gif") {
            continue;
        }
        let id = match item.get("slug").or_else(|| item.get("id")) {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        let Some(files) = item.get("file").or_else(|| item.get("files")) else { continue };
        // In sizes, or (an older answer) formats straight away.
        let size = |name: &str, format: &str| rendition(files.get(name).and_then(|s| s.get(format)));
        let flat = |format: &str| rendition(files.get(format));
        let full: Vec<Rendition> =
            ["hd", "md", "sm"].iter().filter_map(|s| size(s, "gif")).chain(flat("gif")).collect();
        let Some(preview) = size("sm", "gif").or_else(|| size("xs", "gif")).or_else(|| full.last().cloned()) else {
            continue;
        };
        let still = ["sm", "xs", "md"].iter().find_map(|s| size(s, "jpg").or_else(|| size(s, "png")));
        found.push(Found {
            id,
            title: item.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
            preview,
            still,
            full,
        });
        if found.len() >= ask.limit as usize {
            break;
        }
    }
    let more = data.get("has_next").and_then(Value::as_bool).unwrap_or(false);
    let next = more.then(|| (ask.cursor + 1).to_string());
    Some(Page { found, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gifs::Kind;

    #[test]
    fn asks_by_page_with_the_key_in_the_path() {
        let setup = Setup { provider: Kind::Klipy, api_key: "app-key_1".into(), ..Default::default() };
        let url = address(None, &setup, Ask { query: "dance", cursor: 2, limit: 1 });
        assert_eq!(url.as_str().split('?').next(), Some("https://api.klipy.com/api/v1/app-key_1/gifs/search"));
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert!(pairs.contains(&("page".into(), "3".into())));
        assert!(pairs.contains(&("per_page".into(), "8".into())));
        assert!(!pairs.iter().any(|(name, _)| name == "customer_id"));
    }

    #[test]
    fn reads_sizes_and_leaves_ads_out() {
        let gif = |w: u32, size: u64| serde_json::json!({"url": format!("https://static.klipy.com/{w}.gif"), "width": w, "height": w / 2, "size": size});
        let body = serde_json::json!({
            "result": true,
            "data": {
                "data": [
                    {"type": "ad", "id": 1},
                    {"id": 42, "slug": "dancing-cat", "title": "Dancing cat", "type": "gif", "file": {
                        "hd": {"gif": gif(498, 4_000_000)},
                        "md": {"gif": gif(320, 1_500_000)},
                        "sm": {"gif": gif(220, 400_000), "jpg": {"url": "https://static.klipy.com/220.jpg", "width": 220, "height": 110}},
                        "xs": {"gif": gif(90, 60_000)}
                    }}
                ],
                "current_page": 1,
                "per_page": 8,
                "has_next": true
            }
        });
        let page = read(body.to_string().as_bytes(), Ask { query: "cat", cursor: 0, limit: 24 }).unwrap();
        assert_eq!(page.found.len(), 1);
        let found = &page.found[0];
        assert_eq!(found.id, "dancing-cat");
        assert_eq!(found.preview.width, 220);
        assert_eq!(found.full.iter().map(|r| r.width).collect::<Vec<_>>(), vec![498, 320, 220]);
        assert_eq!(found.still.as_ref().map(|s| s.url.as_str()), Some("https://static.klipy.com/220.jpg"));
        assert_eq!(page.next.as_deref(), Some("1"));
    }
}
