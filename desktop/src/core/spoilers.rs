//! Spoilers: text between `||` pairs, as in Discord, hidden until the reader
//! reveals it (the web's `lib/spoilers.ts`). Pairs are found within one
//! paragraph and wrap whatever is between them (bold, links, mentions, emoji
//! and line breaks included); code is left alone, and a `||` without a
//! partner, or `||||`, stays as it is.

use std::ops::Range;

const MARK: &str = "||";

/// A spoiler found in Markdown source: the whole of it, markers included,
/// and what's between the markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub whole: Range<usize>,
    pub inner: Range<usize>,
}

/// Whether a line starts a block of its own (a heading, a list item, a quote
/// after a line that wasn't one), which ends the paragraph before it.
fn starts_block(line: &str, previous: Option<&str>) -> bool {
    let t = line.trim_start();
    let list = ["- ", "* ", "+ "].iter().any(|m| t.starts_with(m)) || {
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") "))
    };
    let quote = t.starts_with('>') && !previous.is_some_and(|p| p.trim_start().starts_with('>'));
    let after_heading = previous.is_some_and(|p| p.trim_start().starts_with('#'));
    t.starts_with('#') || list || quote || after_heading
}

/// Every spoiler in `source`, in order.
pub fn spans(source: &str) -> Vec<Span> {
    let mut found = Vec::new();
    // An open marker per bracket depth (a link's words are their own parent, as in the web's tree).
    let mut open: Vec<Option<usize>> = vec![None];
    let mut fenced = false;
    let mut previous: Option<&str> = None;
    let mut at = 0;
    for line in source.split('\n') {
        let start = at;
        at += line.len() + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            open = vec![None];
            previous = None;
            continue;
        }
        if fenced {
            continue;
        }
        if trimmed.is_empty() || starts_block(line, previous) {
            open = vec![None];
        }
        previous = Some(line);
        let bytes = line.as_bytes();
        let (mut i, mut in_code, mut paren) = (0, false, 0usize);
        while i < bytes.len() {
            let c = bytes[i];
            if in_code {
                in_code = c != b'`';
                i += 1;
                continue;
            }
            if paren > 0 {
                match c {
                    b'(' => paren += 1,
                    b')' => paren -= 1,
                    _ => {}
                }
                i += 1;
                continue;
            }
            match c {
                b'\\' => {
                    i += 2;
                    continue;
                }
                b'`' => in_code = true,
                b'[' => open.push(None),
                b']' => {
                    if open.len() > 1 {
                        open.pop();
                    }
                    if bytes.get(i + 1) == Some(&b'(') {
                        paren = 1;
                        i += 2;
                        continue;
                    }
                }
                // An autolink or a token (`<@id>`, `<:name:id>`, `<https://…>`) goes along whole.
                b'<' => {
                    if let Some(close) = line[i..].find('>').filter(|&n| !line[i..i + n].contains(' ')) {
                        i += close + 1;
                        continue;
                    }
                }
                b'|' if line[i..].starts_with(MARK) => {
                    let here = start + i;
                    let slot = open.last_mut().expect("never empty");
                    match slot.take() {
                        Some(from) if here > from + MARK.len() => {
                            found.push(Span { whole: from..here + MARK.len(), inner: from + MARK.len()..here })
                        }
                        // `||||` stays as it is.
                        Some(_) => {}
                        None => *slot = Some(here),
                    }
                    i += MARK.len();
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
    }
    found
}

/// Text with its spoilers blanked out, for places that show a message as
/// plain text (a system notification): each becomes a block of shade about
/// as long, so nothing hidden shows outside the app. The web's `hideSpoilers`.
pub fn hide(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(MARK) {
        let after = &rest[open + MARK.len()..];
        // At least one character hidden, then the nearest closing marker.
        let Some(first) = after.chars().next() else { break };
        let Some(close) = after[first.len_utf8()..].find(MARK).map(|n| n + first.len_utf8()) else { break };
        out.push_str(&rest[..open]);
        let hidden = after[..close].trim().chars().count().clamp(3, 12);
        out.extend(std::iter::repeat_n('▒', hidden));
        rest = &after[close + MARK.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner(source: &str) -> Vec<&str> {
        spans(source).into_iter().map(|s| &source[s.inner]).collect()
    }

    #[test]
    fn pairs_wrap_what_is_between() {
        assert_eq!(inner("a ||b|| c ||d||"), ["b", "d"]);
        assert_eq!(inner("||**bold** [link](https://x) <@01J9>||"), ["**bold** [link](https://x) <@01J9>"]);
        assert_eq!(inner("||one\ntwo||"), ["one\ntwo"]);
        let s = spans("x ||y|| z");
        assert_eq!(s[0].whole, 2..7);
    }

    #[test]
    fn lone_and_empty_markers_stay() {
        assert!(inner("a || b").is_empty());
        assert!(inner("||||").is_empty());
        assert_eq!(inner("||a|| ||"), ["a"]);
        assert!(inner("a ||b\n\nc|| d").is_empty());
        assert!(inner("- ||a\n- b||").is_empty());
    }

    #[test]
    fn code_and_addresses_are_left_alone() {
        assert!(inner("`||a||`").is_empty());
        assert!(inner("```\n||a||\n```").is_empty());
        assert!(inner("[x](https://a.example/||b||)").is_empty());
        assert!(inner("<https://a.example/||b||>").is_empty());
        assert!(inner("\\||a||").is_empty());
        assert_eq!(inner("||https://x.example||"), ["https://x.example"]);
        // A link's words are their own paragraph: markers don't pair across its edge.
        assert!(inner("||[a||](https://x)").is_empty());
        assert_eq!(inner("[||a||](https://x)"), ["a"]);
    }

    #[test]
    fn hidden_in_plain_text() {
        assert_eq!(hide("the ||butler|| did it"), "the ▒▒▒▒▒▒ did it");
        assert_eq!(hide("||a||"), "▒▒▒");
        assert_eq!(hide("||a very long secret indeed||"), "▒".repeat(12));
        assert_eq!(hide("a || b"), "a || b");
        assert_eq!(hide("||||x||"), "▒▒▒");
    }
}
