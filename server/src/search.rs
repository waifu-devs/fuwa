//! Cutting text into the words the search index keeps (`api/search.rs` keeps
//! the index and answers searches). Words are folded so case and accents
//! don't matter: "Café" and "cafe" are one word. Chinese, Japanese and Korean
//! have no spaces between words, so their runs are cut into overlapping pairs
//! of characters ("東京都" is "東京" and "京都"), which finds a run of two or
//! more characters anywhere without a dictionary. Mentions (`@username`, and
//! `<@id>` from webhooks) become words of their own that no text makes; other
//! markup (`<#id>`, `<@&id>`) isn't searchable, and custom emoji count by name.

use crate::pb;

/// The way words are cut. A change that finds different words for the same
/// text bumps it, and every server's index is built again.
pub const VERSION: i64 = 1;

/// Longer words are cut to this many characters, in the index and in searches
/// alike, so they still match.
const MAX_WORD: usize = 32;

/// What a message has, as bits in the index (`search_docs.has`).
pub const HAS_LINK: i64 = 1;
pub const HAS_EMBED: i64 = 2;
pub const HAS_FILE: i64 = 4;
pub const HAS_PICTURE: i64 = 8;
pub const HAS_VIDEO: i64 = 16;
pub const HAS_SOUND: i64 = 32;
pub const HAS_EVERYONE: i64 = 64;

/// The bit for one of the filters a search takes.
pub fn has_bit(has: pb::SearchHas) -> i64 {
    match has {
        pb::SearchHas::Unspecified => 0,
        pb::SearchHas::Link => HAS_LINK,
        pb::SearchHas::Embed => HAS_EMBED,
        pb::SearchHas::File => HAS_FILE,
        pb::SearchHas::Picture => HAS_PICTURE,
        pb::SearchHas::Video => HAS_VIDEO,
        pb::SearchHas::Sound => HAS_SOUND,
        pb::SearchHas::EveryoneMention => HAS_EVERYONE,
    }
}

/// The word a mention is kept under: `@` and the username (or the user id,
/// for `<@id>`). Text never makes it, since words are only letters and digits.
pub fn mention_word(name: &str) -> String {
    format!("@{name}")
}

/// A word found in some text: where it is (bytes) and its folded form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub word: String,
    /// Inside markup (an emoji's name), so not highlighted.
    pub markup: bool,
}

/// Chinese, Japanese and Korean characters, which are cut into pairs.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF       // Hiragana, Katakana
        | 0x31F0..=0x31FF     // Katakana extensions
        | 0x3400..=0x4DBF     // CJK extension A
        | 0x4E00..=0x9FFF     // CJK unified ideographs
        | 0xF900..=0xFAFF     // CJK compatibility ideographs
        | 0xFF66..=0xFF9F     // Half-width Katakana
        | 0x1100..=0x11FF     // Hangul Jamo
        | 0x3130..=0x318F     // Hangul compatibility Jamo
        | 0xAC00..=0xD7AF     // Hangul syllables
        | 0x20000..=0x2FA1F) // CJK extensions B on
}

/// Letters and digits make words; everything else separates them.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// Appends `c` folded: lower case, accents off common Latin letters, and
/// full-width Latin letters and digits as plain ones.
fn fold_into(c: char, out: &mut String) {
    let c = match c as u32 {
        0xFF10..=0xFF19 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    };
    for lower in c.to_lowercase() {
        match plain(lower) {
            Some(s) => out.push_str(s),
            None => out.push(lower),
        }
    }
}

/// Latin letters with accents (and a few ligatures) as plain letters.
fn plain(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' | 'ạ' | 'ả' | 'ấ' | 'ầ' | 'ẩ' | 'ẫ' | 'ậ' | 'ắ'
        | 'ằ' | 'ẳ' | 'ẵ' | 'ặ' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'ẹ' | 'ẻ' | 'ẽ' | 'ế' | 'ề' | 'ể' | 'ễ' | 'ệ' => {
            "e"
        }
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' | 'ỉ' | 'ị' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ǒ' | 'ơ' | 'ọ' | 'ỏ' | 'ố' | 'ồ' | 'ổ' | 'ỗ' | 'ộ'
        | 'ớ' | 'ờ' | 'ở' | 'ỡ' | 'ợ' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ǔ' | 'ư' | 'ụ' | 'ủ' | 'ứ' | 'ừ' | 'ử' | 'ữ'
        | 'ự' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' | 'ỳ' | 'ỵ' | 'ỷ' | 'ỹ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Folds a whole word and cuts it to [`MAX_WORD`] characters.
fn fold(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        fold_into(c, &mut out);
    }
    match out.char_indices().nth(MAX_WORD) {
        Some((cut, _)) => out[..cut].to_string(),
        None => out,
    }
}

/// What a piece of markup in message text is searched by.
enum Markup {
    /// `<@id>` or `<@!id>`: a mention, kept as [`mention_word`].
    Mention(String),
    /// `<:name:id>` or `<a:name:id>`: a custom emoji, found by its name's
    /// words; the name starts this many bytes in and is this long.
    Emoji(usize, usize),
    /// `<#id>` and `<@&id>`: channels and roles, not searched.
    Skip,
}

/// The markup at the start of `rest` (which starts with `<`), and how many
/// bytes it takes.
fn markup(rest: &str) -> Option<(usize, Markup)> {
    let end = rest.get(..rest.len().min(100)).unwrap_or(rest).find('>')?;
    let inner = &rest[1..end];
    let id = |s: &str| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if let Some(user) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@')).filter(|u| id(u)) {
        return Some((end + 1, Markup::Mention(mention_word(user))));
    }
    if inner.strip_prefix("@&").or_else(|| inner.strip_prefix('#')).is_some_and(id) {
        return Some((end + 1, Markup::Skip));
    }
    let (skip, emoji) = match inner.strip_prefix("a:") {
        Some(emoji) => (3, emoji),
        None => (2, inner.strip_prefix(':')?),
    };
    let (name, emoji_id) = emoji.split_once(':')?;
    if !id(emoji_id) || name.is_empty() || !name.chars().all(|c| is_word_char(c) || c == '_') {
        return None;
    }
    Some((end + 1, Markup::Emoji(skip, name.len())))
}

/// The words in `text`, in order, with where each is. Mentions come out as
/// [`mention_word`]s and emoji as their names' words.
pub fn words(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        // `@username`, as the apps write mentions: a word of its own, and the
        // name stays searchable as text too.
        if c == '@'
            && let Some(name) = mentioned(text, start)
        {
            found.push(Found {
                start,
                end: start + 1 + name.len(),
                word: mention_word(&name.to_ascii_lowercase()),
                markup: true,
            });
            chars.next();
            continue;
        }
        if c == '<'
            && let Some((len, markup)) = markup(&text[start..])
        {
            match markup {
                Markup::Mention(word) => found.push(Found { start, end: start + len, word, markup: true }),
                Markup::Emoji(skip, name_len) => {
                    let offset = start + skip;
                    for mut inner in words(&text[offset..offset + name_len]) {
                        inner.start += offset;
                        inner.end += offset;
                        inner.markup = true;
                        found.push(inner);
                    }
                }
                Markup::Skip => {}
            }
            while chars.peek().is_some_and(|&(at, _)| at < start + len) {
                chars.next();
            }
            continue;
        }
        if is_cjk(c) {
            let mut run = Vec::new();
            while let Some(&(at, c)) = chars.peek().filter(|&&(_, c)| is_cjk(c)) {
                run.push((at, c));
                chars.next();
            }
            let end_of = |i: usize| run.get(i + 1).map_or(start_after(text, run[i].0), |&(at, _)| at);
            if run.len() == 1 {
                found.push(Found { start: run[0].0, end: end_of(0), word: fold(&run[0].1.to_string()), markup: false });
            }
            for i in 0..run.len().saturating_sub(1) {
                let word = format!("{}{}", run[i].1, run[i + 1].1);
                found.push(Found { start: run[i].0, end: end_of(i + 1), word: fold(&word), markup: false });
            }
            continue;
        }
        if is_word_char(c) {
            let mut end = start;
            while let Some(&(at, c)) = chars.peek().filter(|&&(_, c)| is_word_char(c) && !is_cjk(c)) {
                end = at + c.len_utf8();
                chars.next();
            }
            let word = fold(&text[start..end]);
            // A mentioned name is found as text too, but drawn as a mention, not highlighted.
            let markup = found.last().is_some_and(|f| f.markup && f.word.starts_with('@') && f.start + 1 == start);
            // One letter is too common to look for.
            if word.chars().count() > 1 {
                found.push(Found { start, end, word, markup });
            }
            continue;
        }
        chars.next();
    }
    found
}

/// The username mentioned by the `@` at `at`, as the apps find them: not
/// right after a letter, digit, `@`, `<` or `.`, then letters, digits, `_` and
/// `.`, not ending in a dot.
fn mentioned(text: &str, at: usize) -> Option<String> {
    let before = text[..at].chars().next_back();
    if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '@' || c == '<' || c == '.') {
        return None;
    }
    let name: String =
        text[at + 1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.').take(32).collect();
    let name = name.trim_end_matches('.');
    let next = text[at + 1 + name.len()..].chars().next();
    if name.is_empty() || next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

/// The byte after the character at `at`.
fn start_after(text: &str, at: usize) -> usize {
    at + text[at..].chars().next().map_or(0, char::len_utf8)
}

/// Every word a message is found by: its text's, its attachments' names',
/// and its embeds' text, once each.
pub fn message_words(message: &pb::Message) -> Vec<String> {
    let mut all: Vec<String> = words(&message.content).into_iter().map(|f| f.word).collect();
    for attachment in &message.attachments {
        all.extend(words(&attachment.filename).into_iter().map(|f| f.word));
    }
    for embed in &message.embeds {
        for text in
            [&embed.title, &embed.description].into_iter().chain(embed.fields.iter().flat_map(|f| [&f.name, &f.value]))
        {
            all.extend(words(text).into_iter().map(|f| f.word));
        }
    }
    all.sort_unstable();
    all.dedup();
    all
}

/// What a message has, as `HAS_*` bits.
pub fn message_has(message: &pb::Message) -> i64 {
    let mut has = 0;
    let lower = message.content.to_ascii_lowercase();
    if lower.contains("http://") || lower.contains("https://") || message.embeds.iter().any(|e| !e.url.is_empty()) {
        has |= HAS_LINK;
    }
    if !message.embeds.is_empty() {
        has |= HAS_EMBED;
    }
    if message.embeds.iter().any(|e| !e.image_url.is_empty() || !e.thumbnail_url.is_empty()) {
        has |= HAS_PICTURE;
    }
    if !message.attachments.is_empty() {
        has |= HAS_FILE;
    }
    for attachment in &message.attachments {
        let kind = attachment.content_type.to_ascii_lowercase();
        if kind.starts_with("image/") {
            has |= HAS_PICTURE;
        } else if kind.starts_with("video/") {
            has |= HAS_VIDEO;
        } else if kind.starts_with("audio/") {
            has |= HAS_SOUND;
        }
    }
    if message.mentions_everyone {
        has |= HAS_EVERYONE;
    }
    has
}

/// One thing a search looks for: any of these words, or (for the last word
/// typed) any word starting with `prefix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub word: String,
    /// Also matches longer words starting with `word`.
    pub prefix: bool,
}

impl Term {
    pub fn matches(&self, word: &str) -> bool {
        if self.prefix { word.starts_with(&self.word) } else { word == self.word }
    }
}

/// The shortest last word that also matches longer words.
const MIN_PREFIX: usize = 3;

/// What a search's words look for, each of which a message must have. The
/// last word also matches words it starts, when it's long enough and the
/// query doesn't end in a space.
pub fn query_terms(query: &str) -> Vec<Term> {
    let found = words(query);
    let open = query.chars().last().is_some_and(is_word_char);
    let last_typed = found.last().map(|f| f.end);
    let mut terms: Vec<Term> = Vec::new();
    for f in found {
        let cjk = f.word.chars().any(is_cjk);
        let prefix = open && Some(f.end) == last_typed && !cjk && f.word.chars().count() >= MIN_PREFIX;
        // Mentions are searched with their filter, not typed.
        if f.word.starts_with('@') {
            continue;
        }
        if !terms.iter().any(|t| t.word == f.word && t.prefix == prefix) {
            terms.push(Term { word: f.word, prefix });
        }
    }
    terms
}

/// Whether a message has every term (looked up again for each result, so a
/// message edited a moment ago never shows for words it no longer has).
pub fn has_all(message: &pb::Message, terms: &[Term]) -> bool {
    let words = message_words(message);
    terms.iter().all(|t| words.iter().any(|w| t.matches(w)))
}

/// Where the terms are in `text`, as UTF-16 ranges, joined where they touch.
pub fn highlights(text: &str, terms: &[Term]) -> Vec<pb::TextRange> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for f in words(text) {
        if f.markup || !terms.iter().any(|t| t.matches(&f.word)) {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if f.start <= last.1 => last.1 = last.1.max(f.end),
            _ => ranges.push((f.start, f.end)),
        }
    }
    let utf16 = |byte: usize| text[..byte].encode_utf16().count() as i32;
    ranges.into_iter().map(|(start, end)| pb::TextRange { start: utf16(start), end: utf16(end) }).collect()
}

/// Word ids, packed as LEB128 varints.
pub fn pack(ids: &[i64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ids.len() * 3);
    for &id in ids {
        let mut v = id as u64;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    out
}

/// Word ids packed by [`pack`]. Stops at a cut-off end.
pub fn unpack(bytes: &[u8]) -> Vec<i64> {
    let mut out = Vec::new();
    let (mut v, mut shift) = (0u64, 0);
    for &byte in bytes {
        if shift < 64 {
            v |= u64::from(byte & 0x7f) << shift;
        }
        shift += 7;
        if byte & 0x80 == 0 {
            out.push(v as i64);
            (v, shift) = (0, 0);
        }
    }
    out
}

/// Where a message sorts in the index: the millisecond it was sent, times
/// 65536, plus a bit of its id so two sent in one millisecond rarely clash
/// (the index steps past a clash).
pub fn doc_of(message_id: &str, sent_ms: i64) -> i64 {
    // FNV-1a, which is the same everywhere and needs nothing.
    let mut hash: u32 = 0x811c9dc5;
    for b in message_id.bytes() {
        hash = (hash ^ u32::from(b)).wrapping_mul(0x01000193);
    }
    sent_ms.max(0).saturating_mul(65536).saturating_add(i64::from(hash & 0xffff))
}

/// The lowest doc of a message sent at `ms`.
pub fn doc_floor(ms: i64) -> i64 {
    ms.max(0).saturating_mul(65536)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn just_words(text: &str) -> Vec<String> {
        words(text).into_iter().map(|f| f.word).collect()
    }

    #[test]
    fn folds_case_and_accents() {
        assert_eq!(just_words("Café CRÈME, naïve façade!"), ["cafe", "creme", "naive", "facade"]);
        assert_eq!(just_words("Straße Ｆｕｗａ ２０２６"), ["strasse", "fuwa", "2026"]);
        assert_eq!(just_words("a b cd"), ["cd"], "one letter is left out");
    }

    #[test]
    fn cuts_cjk_into_pairs() {
        assert_eq!(just_words("東京都に行く"), ["東京", "京都", "都に", "に行", "行く"]);
        assert_eq!(just_words("猫 cat"), ["猫", "cat"]);
        let found = words("hi 東京");
        assert_eq!(&"hi 東京"[found[1].start..found[1].end], "東京");
    }

    #[test]
    fn mentions_and_emoji() {
        assert_eq!(
            just_words("hey <@01ABC> look <#01CH> <@&01RO> <:party_time:01EM>"),
            ["hey", "@01ABC", "look", "party", "time"]
        );
        assert_eq!(just_words("<@!01ABC>"), ["@01ABC"]);
        assert_eq!(just_words("@Mika_chan. hi @here"), ["@mika_chan", "mika", "chan", "hi", "@here", "here"]);
        assert_eq!(just_words("mail@example.com"), ["mail", "example", "com"], "an address mentions nobody");
        assert_eq!(just_words("<a:wave:01EM>"), ["wave"]);
        let found = words("<:big_cat:01EM>");
        assert_eq!(&"<:big_cat:01EM>"[found[1].start..found[1].end], "cat");
        assert_eq!(just_words("x <b> y <@> <:x:>"), Vec::<String>::new());
    }

    #[test]
    fn long_words_are_cut() {
        let long = "x".repeat(50);
        assert_eq!(just_words(&long), ["x".repeat(32)]);
        assert_eq!(query_terms(&format!("{long} ")), [Term { word: "x".repeat(32), prefix: false }]);
    }

    #[test]
    fn query_terms_prefix_the_last_word() {
        assert_eq!(
            query_terms("hello wor"),
            [Term { word: "hello".into(), prefix: false }, Term { word: "wor".into(), prefix: true }]
        );
        assert_eq!(
            query_terms("hello wor "),
            [Term { word: "hello".into(), prefix: false }, Term { word: "wor".into(), prefix: false }]
        );
        assert_eq!(
            query_terms("hello wo"),
            [Term { word: "hello".into(), prefix: false }, Term { word: "wo".into(), prefix: false }]
        );
        assert_eq!(query_terms("東京"), [Term { word: "東京".into(), prefix: false }]);
        assert!(query_terms("<@01ABC>").is_empty());
    }

    #[test]
    fn highlights_in_utf16() {
        let terms = query_terms("cafe wor");
        // "é" is one UTF-16 unit; "🌸" is two.
        let text = "🌸 Café world, words café";
        let ranges: Vec<(i32, i32)> = highlights(text, &terms).iter().map(|r| (r.start, r.end)).collect();
        assert_eq!(ranges, [(3, 7), (8, 13), (15, 20), (21, 25)]);
        let utf16: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&utf16[3..7]).unwrap(), "Café");
        // An emoji's name finds the message but isn't marked inside its markup.
        assert!(highlights("<:party:01EM> party", &query_terms("party ")).iter().all(|r| r.start >= 14));
    }

    #[test]
    fn what_messages_have() {
        let message = pb::Message {
            content: "see HTTPS://example.com".into(),
            attachments: vec![pb::Attachment {
                content_type: "image/png".into(),
                filename: "Cat Photo.png".into(),
                ..Default::default()
            }],
            embeds: vec![pb::Embed { title: "Release notes".into(), ..Default::default() }],
            mentions_everyone: true,
            ..Default::default()
        };
        assert_eq!(message_has(&message), HAS_LINK | HAS_EMBED | HAS_FILE | HAS_PICTURE | HAS_EVERYONE);
        assert_eq!(
            message_words(&message),
            ["cat", "com", "example", "https", "notes", "photo", "png", "release", "see"]
        );
        assert!(has_all(&message, &query_terms("photo rele")));
        assert!(!has_all(&message, &query_terms("photo dog")));
    }

    #[test]
    fn packs_word_ids() {
        let ids = [0, 1, 127, 128, 300, 16384, i64::from(u32::MAX), 1 << 40];
        assert_eq!(unpack(&pack(&ids)), ids);
        assert_eq!(pack(&[1, 2, 3]).len(), 3);
    }

    #[test]
    fn docs_sort_by_time() {
        assert!(doc_of("B", 1001) > doc_of("A", 1000));
        assert!(doc_of("A", 1000) >= doc_floor(1000) && doc_of("A", 1000) < doc_floor(1001));
    }
}
