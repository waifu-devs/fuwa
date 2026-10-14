//! Link previews (docs/link-previews.md): the pages links in messages point
//! to, read by the instance so a card with their title, description and
//! picture can go under the message.
//!
//! Apps never fetch a linked page or its picture themselves, which would tell
//! the site each reader's IP address: the instance reads the page once, as
//! itself, and the picture goes through its proxy (`outside.rs`) like every
//! other picture from elsewhere. Pages are read only at public addresses
//! (unless FUWA_LINK_PREVIEWS_ALLOW_PRIVATE), only their head and at most
//! [`MAX_PAGE_BYTES`], and what they said is kept in memory for a while so
//! a link pasted in many places is read once.
//!
//! [`links`] and [`read_page`] are pure; [`page`] fetches.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use http::header;
use reqwest::Url;
use tokio::sync::Semaphore;

/// Most links in one message that get a preview.
pub const MAX_PREVIEWS: usize = 3;

/// The most of a page read: its head is nearly always well within it.
const MAX_PAGE_BYTES: usize = 512 * 1024;

/// How long reading one page may take, redirects and all.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 5;

/// Pages read at once; more wait their turn.
const MAX_FETCHES: usize = 16;

/// How long what a page said is kept, and how many pages are.
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const CACHE_ENTRIES: usize = 2048;

/// The longest title, description and site name kept, in characters.
const MAX_TITLE: usize = 256;
const MAX_DESCRIPTION: usize = 350;
const MAX_SITE_NAME: usize = 100;

/// What a linked page says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub title: String,
    pub description: String,
    pub site_name: String,
    /// The page's picture, made absolute against the page's address.
    pub image: String,
    /// The page asks for its picture to be shown big (a
    /// `summary_large_image` card, or the link was a picture itself).
    pub large: bool,
    /// Its theme color, as 0xRRGGBB.
    pub color: Option<i32>,
}

impl Page {
    /// Whether there's anything to show.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.description.is_empty() && self.image.is_empty()
    }
}

/// The links in a message's text that get a preview, in order, at most
/// [`MAX_PREVIEWS`] and each once: http(s) links, bare or in a Markdown
/// link, but not in code, behind a spoiler or wrapped in `<...>` (the way
/// to send a link without a preview).
pub fn links(content: &str) -> Vec<String> {
    let text = outside_code(content);
    let mut found: Vec<String> = Vec::new();
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut spoiler = false;
    let mut at = 0;
    while at < text.len() && found.len() < MAX_PREVIEWS {
        if lower[at..].starts_with("||") {
            spoiler = !spoiler;
            at += 2;
            continue;
        }
        let starts = lower[at..].starts_with("https://") || lower[at..].starts_with("http://");
        let word_start = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        if !starts || !word_start {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let end = text[at..]
            .find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '|')
            .map_or(text.len(), |n| at + n);
        let raw = trim_link(&text[at..end]);
        let wrapped = at > 0 && bytes[at - 1] == b'<';
        if !spoiler
            && !wrapped
            && let Ok(url) = Url::parse(raw)
            && url.host_str().is_some()
        {
            let link = url.to_string();
            if !found.contains(&link) {
                found.push(link);
            }
        }
        at = end.max(at + 1);
    }
    found
}

/// The text with code blocks and code spans blanked out (same length, so
/// nothing in them looks like a link).
fn outside_code(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let ticks = rest[start..].chars().take_while(|&c| c == '`').count();
        let fence = &rest[start..start + ticks];
        let after = &rest[start + ticks..];
        match after.find(fence) {
            Some(close) => {
                let skipped = ticks + close + ticks;
                out.extend(std::iter::repeat_n(' ', skipped));
                rest = &rest[start + skipped..];
            }
            None => {
                out.push_str(fence);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A link without what usually follows one in a sentence: closing
/// punctuation, and a closing bracket it didn't open (a Markdown link's,
/// or the text's own).
fn trim_link(raw: &str) -> &str {
    let mut link = raw;
    loop {
        let Some(last) = link.chars().last() else { return link };
        let trailing = matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '_' | '~')
            || (last == ')' && link.matches('(').count() < link.matches(')').count())
            || (last == ']' && link.matches('[').count() < link.matches(']').count());
        if !trailing {
            return link;
        }
        link = &link[..link.len() - last.len_utf8()];
    }
}

/// What a page's head says about it: its OpenGraph and Twitter card tags,
/// else its `<title>` and description. `base` is where it was read, for
/// pictures given relative to it.
pub fn read_page(html: &str, base: &Url) -> Page {
    let head = match find_ci(html, "</head") {
        Some(end) => &html[..end],
        None => html,
    };
    let mut tags: HashMap<String, String> = HashMap::new();
    let mut at = 0;
    while let Some(found) = find_ci(&head[at..], "<meta") {
        let start = at + found + "<meta".len();
        let (attrs, end) = attributes(head, start);
        at = end;
        let key = attrs.get("property").or_else(|| attrs.get("name")).map(|k| k.to_ascii_lowercase());
        if let (Some(key), Some(value)) = (key, attrs.get("content")) {
            tags.entry(key).or_insert_with(|| value.clone());
        }
    }
    let tag = |names: &[&str]| -> String {
        names.iter().filter_map(|name| tags.get(*name)).map(|v| clean(v)).find(|v| !v.is_empty()).unwrap_or_default()
    };
    let title = match tag(&["og:title", "twitter:title"]) {
        found if !found.is_empty() => found,
        _ => title_element(head).unwrap_or_default(),
    };
    let image = tag(&["og:image:secure_url", "og:image:url", "og:image", "twitter:image", "twitter:image:src"]);
    let image = base.join(&image).ok().filter(|u| matches!(u.scheme(), "http" | "https") && !image.is_empty());
    Page {
        title: clip(&title, MAX_TITLE),
        description: clip(&tag(&["og:description", "twitter:description", "description"]), MAX_DESCRIPTION),
        site_name: clip(&tag(&["og:site_name", "application-name"]), MAX_SITE_NAME),
        image: image.map(|u| u.to_string()).unwrap_or_default(),
        large: tag(&["twitter:card"]).eq_ignore_ascii_case("summary_large_image"),
        color: color(&tag(&["theme-color"])),
    }
}

/// Where `needle` (lowercase ASCII) first is in `haystack`, ignoring case.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack.as_bytes().windows(needle.len()).position(|window| window.eq_ignore_ascii_case(needle))
}

/// A tag's attributes from just after its name, and where it ends.
fn attributes(html: &str, mut at: usize) -> (HashMap<String, String>, usize) {
    let bytes = html.as_bytes();
    let mut attrs = HashMap::new();
    loop {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] == b'>' {
            return (attrs, (at + 1).min(bytes.len()));
        }
        let name_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'>' | b'/') {
            at += 1;
        }
        let name = html[name_start..at].to_ascii_lowercase();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = String::new();
        if at < bytes.len() && bytes[at] == b'=' {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if at < bytes.len() && matches!(bytes[at], b'"' | b'\'') {
                let quote = bytes[at];
                let start = at + 1;
                let end = bytes[start..].iter().position(|&b| b == quote).map_or(bytes.len(), |n| start + n);
                value = html[start..end].to_string();
                at = (end + 1).min(bytes.len());
            } else {
                let start = at;
                while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' {
                    at += 1;
                }
                value = html[start..at].to_string();
            }
        }
        if !name.is_empty() {
            attrs.entry(name).or_insert(value);
        } else {
            at += 1;
        }
    }
}

fn title_element(head: &str) -> Option<String> {
    let open = find_ci(head, "<title")?;
    let start = open + head[open..].find('>')? + 1;
    let end = start + find_ci(&head[start..], "</title")?;
    Some(clean(&head[start..end])).filter(|t| !t.is_empty())
}

/// Text from a page: entities decoded, whitespace folded, no control characters.
fn clean(text: &str) -> String {
    let decoded = decode_entities(text);
    let mut out = String::with_capacity(decoded.len());
    for word in decoded.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().filter(|c| !c.is_control()));
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[1..].find(';').map(|n| n + 1).filter(|&n| n <= 10);
        let decoded = end.and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => ' ',
                "hellip" => '…',
                "mdash" => '—',
                "ndash" => '–',
                "rsquo" => '’',
                "lsquo" => '‘',
                "rdquo" => '”',
                "ldquo" => '“',
                _ => {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A CSS color as 0xRRGGBB: `#rgb` or `#rrggbb` only.
fn color(value: &str) -> Option<i32> {
    let hex = value.trim().strip_prefix('#')?;
    let full = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => return None,
    };
    i32::from_str_radix(&full, 16).ok()
}

static FETCHES: Semaphore = Semaphore::const_new(MAX_FETCHES);

type Cache = Mutex<HashMap<String, (Instant, Option<Page>)>>;
static CACHE: LazyLock<Cache> = LazyLock::new(Default::default);

static PUBLIC: LazyLock<reqwest::Client> = LazyLock::new(|| client(false));
static ANYWHERE: LazyLock<reqwest::Client> = LazyLock::new(|| client(true));

fn client(allow_private: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        // Sites tell crawlers that want a preview apart by "compatible".
        .user_agent(format!(
            "Mozilla/5.0 (compatible; fuwa/{}; link preview; +https://github.com/waifu-devs/fuwa)",
            crate::VERSION
        ))
        // Never through a proxy from the environment: it would resolve
        // names itself, past the check on where they point.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if let (false, Err(reason)) = (allow_private, crate::outside::fetchable(attempt.url())) {
                attempt.error(reason)
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(Duration::from_secs(4))
        .timeout(FETCH_TIMEOUT);
    let builder = if allow_private { builder } else { builder.dns_resolver(Arc::new(crate::outside::PublicOnly)) };
    builder.build().expect("the link preview fetcher's settings are valid")
}

/// What the page at `url` says about itself, from the cache or read now;
/// `None` when it can't be read or isn't a page (or a picture).
pub async fn page(url: &str, allow_private: bool) -> Option<Page> {
    if let Some((at, page)) = CACHE.lock().unwrap().get(url)
        && at.elapsed() < CACHE_TTL
    {
        return page.clone();
    }
    let page = fetch(url, allow_private).await.ok().filter(|page| !page.is_empty());
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= CACHE_ENTRIES {
        cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        if cache.len() >= CACHE_ENTRIES {
            cache.clear();
        }
    }
    cache.insert(url.to_string(), (Instant::now(), page.clone()));
    page
}

async fn fetch(url: &str, allow_private: bool) -> Result<Page, &'static str> {
    let parsed = Url::parse(url).map_err(|_| "not a link")?;
    if !allow_private {
        crate::outside::fetchable(&parsed)?;
    }
    let _turn = FETCHES.acquire().await.map_err(|_| "closed")?;
    let client = if allow_private { &*ANYWHERE } else { &*PUBLIC };
    let response = client
        .get(parsed)
        .header(header::ACCEPT, "text/html,application/xhtml+xml;q=0.9,image/*;q=0.8,*/*;q=0.1")
        .send()
        .await
        .map_err(|_| "the site didn't answer")?;
    if !response.status().is_success() {
        return Err("the site said no");
    }
    let base = response.url().clone();
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().trim().to_ascii_lowercase())
        .unwrap_or_default();
    if kind.starts_with("image/") {
        // A picture itself: shown big, through the proxy, which checks it.
        return Ok(Page { image: url.to_string(), large: true, ..Default::default() });
    }
    if kind != "text/html" && kind != "application/xhtml+xml" {
        return Err("not a page");
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "cut off")?;
        let room = MAX_PAGE_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        // The head is all that's read: stop once it's over.
        if body.len() >= MAX_PAGE_BYTES || find_ci(&String::from_utf8_lossy(&body), "</head").is_some() {
            break;
        }
    }
    Ok(read_page(&String::from_utf8_lossy(&body), &base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_but_not_in_code_spoilers_or_brackets() {
        assert_eq!(
            links("look https://example.com/a, and http://x.org/b."),
            ["https://example.com/a", "http://x.org/b"]
        );
        assert_eq!(links("`https://code.example` and ```\nhttps://block.example\n```"), Vec::<String>::new());
        assert_eq!(links("<https://quiet.example> ||https://spoiler.example||"), Vec::<String>::new());
        assert_eq!(
            links("[docs](https://example.com/docs) (see https://en.wikipedia.org/wiki/Rust_(language))"),
            ["https://example.com/docs", "https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(
            links("https://a.example https://a.example https://b.example https://c.example https://d.example"),
            ["https://a.example/", "https://b.example/", "https://c.example/"]
        );
        assert_eq!(links("xhttps://nope.example ftp://nope.example"), Vec::<String>::new());
    }

    #[test]
    fn reads_open_graph_tags() {
        let html = r##"<!doctype html><html><head>
            <title>Fallback &amp; title</title>
            <meta property="og:title" content="The  &quot;real&quot;
               title">
            <META name=description content='A page &#x2014; about things'>
            <meta property="og:site_name" content="Example">
            <meta property="og:image" content="/pic.png" />
            <meta name="twitter:card" content="summary_large_image">
            <meta name="theme-color" content="#f0a">
            </head><body><meta property="og:title" content="not this"></body>"##;
        let page = read_page(html, &Url::parse("https://example.com/post/1").unwrap());
        assert_eq!(page.title, "The \"real\" title");
        assert_eq!(page.description, "A page — about things");
        assert_eq!(page.site_name, "Example");
        assert_eq!(page.image, "https://example.com/pic.png");
        assert!(page.large);
        assert_eq!(page.color, Some(0xFF00AA));
    }

    #[test]
    fn falls_back_to_the_title_and_clips() {
        let long = "word ".repeat(200);
        let html = format!("<head><title>\n  Just a title </title><meta name='description' content='{long}'>");
        let page = read_page(&html, &Url::parse("https://example.com/").unwrap());
        assert_eq!(page.title, "Just a title");
        assert!(page.description.chars().count() <= MAX_DESCRIPTION && page.description.ends_with('…'));
        assert!(page.image.is_empty() && !page.large && page.color.is_none());
        let bad =
            read_page("<meta property='og:image' content='javascript:alert(1)'>", &Url::parse("https://e.x/").unwrap());
        assert!(bad.image.is_empty());
    }
}
