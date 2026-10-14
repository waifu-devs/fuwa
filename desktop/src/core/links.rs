//! Masked links: Markdown links whose words aren't their address
//! (`[free nitro](https://phish.example)`). Opening one asks first, showing
//! where it really goes (`ui/leaving.rs`), unless its site is one the person
//! trusts (`Prefs::trusted_domains`, this computer's only). Bare links and
//! links whose words are their own address open at once. The web's
//! `lib/links.ts`.

use url::Url;

/// Most sites kept in the trusted list.
pub const MAX_TRUSTED: usize = 500;

fn has_scheme(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && text
            .split_once(':')
            .is_some_and(|(scheme, _)| scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)))
}

fn parse(text: &str) -> Option<Url> {
    if has_scheme(text) { Url::parse(text) } else { Url::parse(&format!("https://{text}")) }.ok()
}

/// The host a link goes to, lowercased, without "www."; empty when it has none.
pub fn host_of(href: &str) -> String {
    let Some(host) = parse(href).and_then(|u| u.host_str().map(str::to_ascii_lowercase)) else { return String::new() };
    host.strip_prefix("www.").map(str::to_owned).unwrap_or(host)
}

/// Whether a link's words hide where it goes: they aren't its address, nor
/// its address without the scheme or a trailing slash. Words that are another
/// address entirely (`[https://bank.example](https://phish.example)`) are the
/// worst kind, and masked too.
pub fn is_masked(words: &str, href: &str) -> bool {
    let said = words.trim();
    if said.is_empty() || said == href {
        return false;
    }
    let (Some(a), Some(b)) = (parse(said), parse(href)) else { return true };
    if !said.contains(['.', '/']) {
        return true;
    }
    // Words without a scheme name the address without one.
    if has_scheme(said) && a.scheme() != b.scheme() {
        return true;
    }
    let plain = |u: &Url| {
        let host = match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            _ => String::new(),
        };
        let query = u.query().map(|q| format!("?{q}")).unwrap_or_default();
        let hash = u.fragment().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default();
        format!("{host}{}{query}{hash}", u.path().trim_end_matches('/'))
    };
    plain(&a) != plain(&b)
}

/// Whether `host` is one of the trusted sites, or under one.
pub fn trusted(domains: &[String], host: &str) -> bool {
    !host.is_empty() && domains.iter().any(|d| host == d || host.ends_with(&format!(".{d}")))
}

/// A site as kept in the trusted list: its host, lowercased, without "www."; empty if it isn't one.
pub fn clean_domain(input: &str) -> String {
    host_of(input.trim())
}

/// The trusted list as kept: each a clean host, once, at most [`MAX_TRUSTED`].
pub fn tidy_domains(domains: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in domains.iter().map(|d| clean_domain(d)).filter(|d| !d.is_empty()) {
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out.truncate(MAX_TRUSTED);
    out
}

/// Whether a click on `href` in the Markdown `source` is on a masked link:
/// some `[words](href)` in it whose words aren't its address. A link not
/// found written out (a reference link, an address the parser rewrote) counts
/// as masked unless the address is written bare, so nothing slips by.
pub fn masked_in(source: &str, href: &str) -> bool {
    let mut seen = false;
    let mut fenced = false;
    for line in source.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // A reference's definition (`[1]: https://…`): the links using it have words of their own.
        if trimmed.starts_with('[') && trimmed.contains("]:") && line.contains(href) {
            return true;
        }
        for (words, url) in written_links(line) {
            if url == href || url.trim_matches(['<', '>']) == href {
                if is_masked(&words, href) {
                    return true;
                }
                seen = true;
            }
        }
    }
    !seen && !source.contains(href)
}

/// The `[words](address)` links on a line, outside code, words as written
/// (without the marks around them, roughly as read).
fn written_links(line: &str) -> Vec<(String, String)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut opens: Vec<usize> = Vec::new();
    let (mut i, mut in_code) = (0, false);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => in_code = !in_code,
            b'[' if !in_code => opens.push(i),
            b']' if !in_code => {
                if let Some(open) = opens.pop()
                    && bytes.get(i + 1) == Some(&b'(')
                {
                    let rest = &line[i + 2..];
                    let mut depth = 1usize;
                    let end = rest.char_indices().find_map(|(n, c)| {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        (depth == 0).then_some(n)
                    });
                    if let Some(end) = end {
                        // The address, without a title after it.
                        let target = rest[..end].trim();
                        let url = target.split_once(char::is_whitespace).map_or(target, |(u, _)| u);
                        let words: String =
                            line[open + 1..i].chars().filter(|c| !matches!(c, '*' | '_' | '~' | '`')).collect();
                        // An image's words are its alt text: not a link's.
                        if !(open > 0 && bytes[open - 1] == b'!') {
                            found.push((words, url.to_owned()));
                        }
                        i += 2 + end + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_whose_words_are_their_address_are_not_masked() {
        assert!(!is_masked("https://example.com", "https://example.com"));
        assert!(!is_masked("example.com", "https://example.com/"));
        assert!(!is_masked("www.example.com/a", "http://www.example.com/a"));
        assert!(!is_masked("https://example.com/", "https://example.com"));
    }

    #[test]
    fn words_that_are_not_the_address_are() {
        assert!(is_masked("free nitro", "https://phish.example"));
        assert!(is_masked("https://bank.example", "https://phish.example"));
        assert!(is_masked("example.com", "https://example.com.phish.example"));
        assert!(is_masked("http://example.com", "https://example.com"));
    }

    #[test]
    fn trusting_a_site_trusts_what_is_under_it() {
        assert_eq!(host_of("https://WWW.Example.com/x"), "example.com");
        assert_eq!(clean_domain(" https://docs.example.com/page "), "docs.example.com");
        assert_eq!(clean_domain("not a site at all"), "");
        let list = vec!["example.com".to_owned()];
        assert!(trusted(&list, "docs.example.com"));
        assert!(trusted(&list, "example.com"));
        assert!(!trusted(&list, "badexample.com"));
        assert!(!trusted(&[], ""));
        assert_eq!(
            tidy_domains(vec!["Example.com".into(), "https://www.example.com/".into(), "nope nope".into()]),
            ["example.com"]
        );
    }

    #[test]
    fn masked_links_are_found_in_the_source() {
        assert!(masked_in("see [click](https://phish.example)", "https://phish.example"));
        assert!(!masked_in("see https://example.com", "https://example.com"));
        assert!(!masked_in("[example.com](https://example.com) and **[x](https://a.example)**", "https://example.com"));
        assert!(masked_in("**[x](https://a.example \"t\")**", "https://a.example"));
        assert!(!masked_in("[https://a.example](<https://a.example>)", "https://a.example"));
        // Written by reference, or not at all: asked about.
        assert!(masked_in("[x][1]\n\n[1]: https://b.example", "https://b.example"));
        assert!(masked_in("[x](https://b.example/%20)", "https://c.example"));
        assert!(!masked_in("`[x](https://a.example)` https://a.example", "https://a.example"));
    }
}
