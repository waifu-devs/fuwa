//! AutoMod: what a server's rules make of a message. Only the matching lives
//! here; `api/automod.rs` keeps the rules and `api/messages.rs` acts on what
//! they catch.

use crate::pb::{self, AutoModTrigger as Trigger};

/// What one rule caught in a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The words or sites that set it off, or the number of pings.
    pub matched: Vec<String>,
}

/// Whether `rule` catches `content`, ignoring whether it's on or who wrote it.
pub fn check(rule: &pb::AutoModRule, content: &str) -> Option<Hit> {
    let matched = match Trigger::try_from(rule.trigger).unwrap_or(Trigger::Unspecified) {
        Trigger::Keywords => keyword_hits(&rule.keywords, &rule.allowed, content),
        Trigger::MentionSpam => {
            let pings = pings(content);
            if rule.mention_limit > 0 && pings > rule.mention_limit as usize {
                vec![format!("{pings} pings")]
            } else {
                vec![]
            }
        }
        Trigger::Links => link_hits(&rule.allowed, content),
        Trigger::Unspecified => vec![],
    };
    (!matched.is_empty()).then_some(Hit { matched })
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The whole word around `start..end` in `text`.
fn word_around(text: &str, start: usize, end: usize) -> &str {
    let from = text[..start].char_indices().rev().take_while(|&(_, c)| is_word(c)).last().map_or(start, |(i, _)| i);
    let to = text[end..].char_indices().find(|&(_, c)| !is_word(c)).map_or(text.len(), |(i, _)| end + i);
    &text[from..to]
}

/// The keywords' matches in `content`, as the words they matched, once each.
/// A keyword is matched whole unless a `*` at its start or end lets it run on.
fn keyword_hits(keywords: &[String], allowed: &[String], content: &str) -> Vec<String> {
    let text = content.to_lowercase();
    let allowed: Vec<String> = allowed.iter().map(|a| a.trim().to_lowercase()).collect();
    let mut hits: Vec<String> = Vec::new();
    for keyword in keywords {
        let keyword = keyword.trim().to_lowercase();
        let open_start = keyword.starts_with('*');
        let open_end = keyword.len() > 1 && keyword.ends_with('*');
        let needle = keyword.trim_matches('*');
        if needle.is_empty() {
            continue;
        }
        for (start, _) in text.match_indices(needle) {
            let end = start + needle.len();
            let starts_word =
                text[..start].chars().next_back().is_none_or(|c| !is_word(c)) || !needle.starts_with(is_word);
            let ends_word = text[end..].chars().next().is_none_or(|c| !is_word(c)) || !needle.ends_with(is_word);
            if !(open_start || starts_word) || !(open_end || ends_word) {
                continue;
            }
            // A phrase is shown as it was matched; a single word as the whole word it's in.
            let shown =
                if needle.contains(char::is_whitespace) { &text[start..end] } else { word_around(&text, start, end) };
            if allowed.iter().any(|a| a == shown) {
                continue;
            }
            if !hits.iter().any(|h| h == shown) {
                hits.push(shown.to_string());
            }
        }
    }
    hits
}

/// How many pings a message carries: the people (`@name`) and roles
/// (`<@&id>`) it names, once each, and @everyone or @here.
pub fn pings(content: &str) -> usize {
    let mut names: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let name_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'.';
    let mut everyone = false;
    for (at, _) in content.match_indices('@') {
        if at > 0 && (name_byte(bytes[at - 1]) || matches!(bytes[at - 1], b'@' | b'<')) {
            continue;
        }
        let rest = &content[at + 1..];
        let len = rest.bytes().take_while(|&b| name_byte(b)).count();
        let name = rest[..len].trim_end_matches('.').to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if name == "everyone" || name == "here" {
            everyone = true;
        } else if !names.contains(&name) {
            names.push(name);
        }
    }
    let mut roles: Vec<&str> = Vec::new();
    for (start, _) in content.match_indices("<@&") {
        let rest = &content[start + 3..];
        if let Some(end) = rest.find('>')
            && end > 0
            && end <= 32
            && rest[..end].bytes().all(|b| b.is_ascii_alphanumeric())
            && !roles.iter().any(|r| r.eq_ignore_ascii_case(&rest[..end]))
        {
            roles.push(&rest[..end]);
        }
    }
    names.len() + roles.len() + usize::from(everyone)
}

/// A site as people list it: no scheme, path or leading `*.`, lowercase.
pub fn site(entry: &str) -> String {
    let entry = entry.trim().to_lowercase();
    let entry = entry.split_once("://").map_or(entry.as_str(), |(_, rest)| rest);
    let entry = entry.split(['/', '?', '#']).next().unwrap_or_default();
    let entry = entry.rsplit_once('@').map_or(entry, |(_, host)| host);
    let entry = entry.split(':').next().unwrap_or_default();
    entry.trim_start_matches("*.").trim_matches('.').to_string()
}

/// The hosts of the links in `content`: `http(s)://...` and `www....`.
pub fn links(content: &str) -> Vec<String> {
    let text = content.to_lowercase();
    let mut hosts: Vec<String> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for scheme in ["http://", "https://"] {
        starts.extend(text.match_indices(scheme).map(|(i, _)| i + scheme.len()));
    }
    for (i, _) in text.match_indices("www.") {
        let after_scheme = text[..i].ends_with("://");
        let own_word = text[..i].chars().next_back().is_none_or(|c| !is_word(c) && c != '.');
        if !after_scheme && own_word {
            starts.push(i);
        }
    }
    starts.sort_unstable();
    for start in starts {
        let rest = &text[start..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '/' | '?' | '#' | '>' | ')' | ']' | '"' | '\'' | '<' | '|')
            })
            .unwrap_or(rest.len());
        let host = site(&rest[..end]);
        if host.contains('.') && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// Links to sites that aren't allowed (a site allows its subdomains too).
fn link_hits(allowed: &[String], content: &str) -> Vec<String> {
    let allowed: Vec<String> = allowed.iter().map(|a| site(a)).filter(|a| !a.is_empty()).collect();
    links(content)
        .into_iter()
        .filter(|host| !allowed.iter().any(|a| host == a || host.ends_with(&format!(".{a}"))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords(words: &[&str], allowed: &[&str]) -> pb::AutoModRule {
        pb::AutoModRule {
            trigger: Trigger::Keywords as i32,
            keywords: words.iter().map(|w| w.to_string()).collect(),
            allowed: allowed.iter().map(|w| w.to_string()).collect(),
            ..Default::default()
        }
    }

    fn hits(rule: &pb::AutoModRule, content: &str) -> Vec<String> {
        check(rule, content).map(|h| h.matched).unwrap_or_default()
    }

    #[test]
    fn keywords_match_whole_words_unless_wildcarded() {
        let rule = keywords(&["cat"], &[]);
        assert_eq!(hits(&rule, "my CAT!"), ["cat"]);
        assert!(hits(&rule, "bobcat catapult").is_empty());
        assert_eq!(hits(&keywords(&["*cat"], &[]), "a bobcat catapult"), ["bobcat"]);
        assert_eq!(hits(&keywords(&["cat*"], &[]), "a bobcat catapult"), ["catapult"]);
        assert_eq!(hits(&keywords(&["*cat*"], &[]), "a bobcat catapult"), ["bobcat", "catapult"]);
        assert_eq!(hits(&keywords(&["free nitro"], &[]), "get FREE  nitro or free nitro now"), ["free nitro"]);
        assert_eq!(hits(&keywords(&["ñoño"], &[]), "eres ÑOÑO."), ["ñoño"]);
    }

    #[test]
    fn allowed_words_never_match() {
        let rule = keywords(&["*ass*"], &["class", "assistant"]);
        assert!(hits(&rule, "my class assistant").is_empty());
        assert_eq!(hits(&rule, "my class is a mass"), ["mass"]);
    }

    #[test]
    fn counts_pings() {
        assert_eq!(pings("@ana @bo @ana <@&01ABC> <@&01abc> @everyone mail@x.com"), 4);
        assert_eq!(pings("@here @everyone"), 1);
        assert_eq!(pings("no pings. @"), 0);
        let rule = pb::AutoModRule { trigger: Trigger::MentionSpam as i32, mention_limit: 2, ..Default::default() };
        assert_eq!(hits(&rule, "@a @b @c"), ["3 pings"]);
        assert!(hits(&rule, "@a @b").is_empty());
    }

    #[test]
    fn finds_links_and_allows_sites() {
        assert_eq!(
            links("see https://Example.com/x, (http://a.b.org) www.c.net and https://user@d.io:8080/ ok.go"),
            ["example.com", "a.b.org", "www.c.net", "d.io"]
        );
        let rule = pb::AutoModRule {
            trigger: Trigger::Links as i32,
            allowed: vec!["https://waifu.dev/".into(), "*.github.com".into()],
            ..Default::default()
        };
        assert!(hits(&rule, "https://waifu.dev https://www.waifu.dev https://gist.github.com").is_empty());
        assert_eq!(hits(&rule, "https://notwaifu.dev https://evil.com"), ["notwaifu.dev", "evil.com"]);
        assert_eq!(site(" HTTPS://*.Foo.com/bar "), "foo.com");
    }
}
