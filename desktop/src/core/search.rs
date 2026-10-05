//! Searching a server's messages (SearchService), read the way the web app's
//! `lib/search-query.ts` and `fuwa/search.ts` read it: words, plus filters
//! written `key:value` anywhere among them (`from:mika in:general has:link
//! before:2026-10-01 cake`). Members and channels are turned into ids here;
//! the instance only ever gets ids, times and words. Recent searches stay on
//! this computer, never on an instance.

use std::ops::Range;

use chrono::{Datelike as _, Local, NaiveDate, TimeZone as _};
use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::store::{InstanceState, user_name};
use crate::pb;
use crate::rpc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterKey {
    From,
    In,
    Mentions,
    Has,
    Before,
    During,
    After,
}

impl FilterKey {
    /// Every filter, in the order the suggestions list them, with what each takes.
    pub const ALL: [(FilterKey, &'static str); 7] = [
        (FilterKey::From, "a member"),
        (FilterKey::In, "a channel"),
        (FilterKey::Mentions, "a member"),
        (FilterKey::Has, "link, embed, file, picture, video, sound"),
        (FilterKey::Before, "a date"),
        (FilterKey::During, "a date"),
        (FilterKey::After, "a date"),
    ];

    pub fn name(self) -> &'static str {
        match self {
            FilterKey::From => "from",
            FilterKey::In => "in",
            FilterKey::Mentions => "mentions",
            FilterKey::Has => "has",
            FilterKey::Before => "before",
            FilterKey::During => "during",
            FilterKey::After => "after",
        }
    }

    pub fn of(name: &str) -> Option<Self> {
        Self::ALL.iter().map(|(k, _)| *k).find(|k| k.name().eq_ignore_ascii_case(name))
    }

    pub fn is_date(self) -> bool {
        matches!(self, FilterKey::Before | FilterKey::During | FilterKey::After)
    }
}

/// What `has:` takes, the words that mean each, and what it asks the instance for.
pub const HAS_VALUES: [(&str, &str, &[&str], pb::SearchHas); 7] = [
    ("link", "link", &["link", "links", "url"], pb::SearchHas::Link),
    ("embed", "embed", &["embed", "embeds"], pb::SearchHas::Embed),
    ("file", "file", &["file", "files", "attachment"], pb::SearchHas::File),
    ("picture", "picture", &["picture", "pictures", "image", "images", "photo"], pb::SearchHas::Picture),
    ("video", "video", &["video", "videos"], pb::SearchHas::Video),
    ("sound", "sound", &["sound", "sounds", "audio"], pb::SearchHas::Sound),
    ("everyone", "@everyone", &["everyone", "@everyone", "here", "@here"], pb::SearchHas::EveryoneMention),
];

/// What a `has:` word means, if anything.
pub fn has_value(word: &str) -> Option<pb::SearchHas> {
    let w = word.to_lowercase();
    HAS_VALUES.iter().find(|(_, _, aliases, _)| aliases.contains(&w.as_str())).map(|(_, _, _, has)| *has)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub key: FilterKey,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
    pub text: String,
    pub filters: Vec<Filter>,
}

/// Splits what was typed into words and filters. A filter with nothing after its colon is just a word for now.
pub fn parse_query(input: &str) -> Parsed {
    let mut words = Vec::new();
    let mut filters = Vec::new();
    for token in input.split_whitespace() {
        let filter = token
            .split_once(':')
            .filter(|(key, value)| !key.is_empty() && !value.is_empty())
            .and_then(|(key, value)| Some(Filter { key: FilterKey::of(key)?, value: value.to_owned() }));
        match filter {
            Some(f) => filters.push(f),
            None => words.push(token),
        }
    }
    // A space at the end means the last word is finished (the instance then matches only that word, not longer ones).
    let mut text = words.join(" ");
    if !words.is_empty() && input.ends_with(char::is_whitespace) {
        text.push(' ');
    }
    Parsed { text, filters }
}

/// The word the caret is in (or right after), as byte offsets, for suggestions.
pub fn token_at(input: &str, caret: usize) -> Range<usize> {
    let caret = caret.min(input.len());
    let start = input[..caret].rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    let end = input[caret..].find(char::is_whitespace).map(|i| caret + i).unwrap_or(input.len());
    start..end
}

/// Puts `replacement` where the word at the caret is, with a space after; the new text and caret.
pub fn replace_token(input: &str, caret: usize, replacement: &str) -> (String, usize) {
    let at = token_at(input, caret);
    let after = input[at.end..].trim_start();
    let next = format!("{}{replacement} {after}", &input[..at.start]);
    (next, at.start + replacement.len() + 1)
}

/// A day typed as 2026-10-04, today or yesterday.
pub fn parse_day(value: &str, today: NaiveDate) -> Option<NaiveDate> {
    match value.to_lowercase().as_str() {
        "today" => Some(today),
        "yesterday" => today.pred_opt(),
        v => {
            let mut parts = v.splitn(3, '-');
            let year = parts.next()?;
            let (month, day) = (parts.next()?, parts.next()?);
            if year.len() != 4 || !(1..=2).contains(&month.len()) || !(1..=2).contains(&day.len()) {
                return None;
            }
            NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)
        }
    }
}

/// The time a search covers, as local days: `before:` a day is up to its
/// start, `after:` a day from the next one's (as Discord reads them), and
/// `during:` that whole day. Several narrow it down. Err is a filter whose date couldn't be read.
pub fn time_range(filters: &[Filter], today: NaiveDate) -> Result<(Option<NaiveDate>, Option<NaiveDate>), Filter> {
    let (mut after, mut before): (Option<NaiveDate>, Option<NaiveDate>) = (None, None);
    for f in filters.iter().filter(|f| f.key.is_date()) {
        let day = parse_day(&f.value, today).ok_or_else(|| f.clone())?;
        let next = day.succ_opt().unwrap_or(day);
        let later = |a: Option<NaiveDate>, b: NaiveDate| Some(a.map_or(b, |a| a.max(b)));
        let earlier = |a: Option<NaiveDate>, b: NaiveDate| Some(a.map_or(b, |a| a.min(b)));
        match f.key {
            FilterKey::Before => before = earlier(before, day),
            FilterKey::After => after = later(after, next),
            _ => {
                after = later(after, day);
                before = earlier(before, next);
            }
        }
    }
    Ok((after, before))
}

/// A day's local midnight, as the instance takes times.
fn midnight(day: NaiveDate) -> Option<prost_types::Timestamp> {
    let at = Local.from_local_datetime(&day.and_hms_opt(0, 0, 0)?).earliest()?;
    Some(prost_types::Timestamp { seconds: at.timestamp(), nanos: 0 })
}

/// Why a filter can't be used, in words.
fn problem(f: &Filter) -> String {
    match f.key {
        FilterKey::From | FilterKey::Mentions => format!("No member here is called {}", f.value),
        FilterKey::In => format!("There's no channel here called #{}", f.value.trim_start_matches('#')),
        FilterKey::Has => "has: takes link, embed, file, picture, video, sound or everyone".into(),
        _ => format!("{}: takes a date like 2026-10-04, today or yesterday", f.key.name()),
    }
}

/// Channels whose messages can be searched.
pub fn searchable(c: &pb::Channel) -> bool {
    matches!(
        pb::ChannelType::try_from(c.r#type),
        Ok(pb::ChannelType::Text | pb::ChannelType::Announcement | pb::ChannelType::Thread)
    )
}

/// The name a member shows as: their nickname here, else their own.
pub fn member_name(m: &pb::Member) -> String {
    match &m.user {
        Some(_) if !m.nickname.is_empty() => m.nickname.clone(),
        Some(user) => user_name(user),
        None => String::new(),
    }
}

/// The member a `from:` or `mentions:` value names: by username, then by the name shown.
pub fn find_member<'a>(members: &'a [pb::Member], value: &str) -> Option<&'a pb::Member> {
    let v = value.trim_start_matches('@').to_lowercase();
    members
        .iter()
        .find(|m| m.user.as_ref().is_some_and(|u| u.username.to_lowercase() == v))
        .or_else(|| members.iter().find(|m| m.user.is_some() && member_name(m).to_lowercase() == v))
}

/// The channel an `in:` value names.
pub fn find_channel<'a>(channels: &'a [pb::Channel], value: &str) -> Option<&'a pb::Channel> {
    let v = value.trim_start_matches('#').to_lowercase();
    channels.iter().find(|c| searchable(c) && c.name.to_lowercase() == v)
}

/// The request for what was typed, with members and channels as ids. None
/// when there's nothing to search for; Err says which filter can't be read.
pub fn request_for(
    i: &InstanceState,
    server_id: &str,
    query: &str,
    today: NaiveDate,
) -> Result<Option<pb::SearchMessagesRequest>, String> {
    let Parsed { text, filters } = parse_query(query);
    let members = i.members.get(server_id).map(Vec::as_slice).unwrap_or_default();
    let channels = i.channels.get(server_id).map(Vec::as_slice).unwrap_or_default();
    let mut request = pb::SearchMessagesRequest { server_id: server_id.to_owned(), ..Default::default() };
    for f in &filters {
        match f.key {
            FilterKey::From | FilterKey::Mentions => {
                let id = find_member(members, &f.value).and_then(|m| m.user.as_ref()).map(|u| u.id.clone());
                let id = id.ok_or_else(|| problem(f))?;
                if f.key == FilterKey::From { request.author_ids.push(id) } else { request.mention_ids.push(id) }
            }
            FilterKey::In => {
                request.channel_ids.push(find_channel(channels, &f.value).ok_or_else(|| problem(f))?.id.clone())
            }
            FilterKey::Has => request.has.push(has_value(&f.value).ok_or_else(|| problem(f))? as i32),
            _ => {}
        }
    }
    let (after, before) = time_range(&filters, today).map_err(|f| problem(&f))?;
    request.after = after.and_then(midnight);
    request.before = before.and_then(midnight);
    if text.trim().is_empty() && filters.is_empty() {
        return Ok(None);
    }
    request.query = text;
    Ok(Some(request))
}

/// Today on this computer's calendar.
pub fn today() -> NaiveDate {
    let now = Local::now();
    NaiveDate::from_ymd_opt(now.year(), now.month(), now.day()).unwrap_or_default()
}

/// Where a server's searches are kept on this computer: one list per
/// instance, account and server, so the next person to sign in here doesn't see them.
pub fn place(key: &str, account_id: &str, server_id: &str) -> String {
    format!("{key}/{account_id}/{server_id}")
}

/// How many recent searches are kept for each server.
pub const MAX_RECENT: usize = 6;

/// The search's matches in `text`, from the instance's UTF-16 ranges to byte
/// ranges; ranges out of order, overlapping or out of bounds are left out.
pub fn byte_ranges(text: &str, ranges: &[pb::TextRange]) -> Vec<Range<usize>> {
    // The byte offset at each UTF-16 offset that starts a character.
    let mut at = Vec::with_capacity(text.len() + 1);
    for (byte, c) in text.char_indices() {
        at.push(Some(byte));
        if c.len_utf16() == 2 {
            at.push(None);
        }
    }
    at.push(Some(text.len()));
    let mut out = Vec::new();
    let mut last = 0;
    for r in ranges {
        let (Ok(start), Ok(end)) = (usize::try_from(r.start), usize::try_from(r.end)) else { continue };
        if end <= start {
            continue;
        }
        let (Some(Some(s)), Some(Some(e))) = (at.get(start), at.get(end)) else { continue };
        if *s < last {
            continue;
        }
        out.push(*s..*e);
        last = *e;
    }
    out
}

/// What one page of a search brought.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub results: Vec<pb::SearchResult>,
    pub total: i64,
    pub total_at_least: bool,
    pub next_cursor: String,
    pub indexing: bool,
    pub indexed_percent: i32,
}

/// How many older pages a jump to a result loads looking for it, as on the web.
const JUMP_PAGES: usize = 100;

impl Core {
    /// One page of a search; its authors join the store, so names and pictures show.
    pub async fn search_page(
        &self,
        key: &str,
        mut request: pb::SearchMessagesRequest,
        cursor: &str,
    ) -> Result<Page, Problem> {
        let api = self.api(key).ok_or_else(|| Problem::new(Code::Unavailable, "That instance isn't here."))?;
        request.cursor = cursor.to_owned();
        let res = rpc!(api.search(), search_messages(request)).await?;
        if !res.authors.is_empty() {
            self.shared.instance(key, |i| {
                for user in res.authors.iter() {
                    i.users.entry(user.id.clone()).or_insert_with(|| user.clone());
                }
            });
        }
        if cursor.is_empty() {
            crate::core::reports::used("search.run");
        }
        Ok(Page {
            results: res.results,
            total: res.total,
            total_at_least: res.total_at_least,
            next_cursor: res.next_cursor,
            indexing: res.indexing,
            indexed_percent: res.indexed_percent,
        })
    }

    /// Loads a channel's older messages until one is there (or there are no
    /// more, or it's too far back). Whether it's there now.
    pub async fn find_message(&self, key: &str, server_id: &str, channel_id: &str, message_id: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let found = || {
            self.shared.read(|s| {
                s.instance(key)
                    .and_then(|i| i.messages.get(channel_id))
                    .is_some_and(|m| m.items.iter().any(|m| m.id == message_id))
            })
        };
        // The oldest message held, so a load that brought nothing (it failed, or the channel is gone) stops the walk.
        let oldest = || {
            self.shared.read(|s| {
                s.instance(key).and_then(|i| i.messages.get(channel_id)).map(|m| m.items.first().map(|m| m.id.clone()))
            })
        };
        let mut pages = 0;
        while !found() && pages < JUMP_PAGES && std::time::Instant::now() < deadline {
            let state = self
                .shared
                .read(|s| s.instance(key).and_then(|i| i.messages.get(channel_id)).map(|m| (m.loading, m.has_more)));
            match state {
                None => {
                    pages += 1;
                    if self.load_messages(key, server_id, channel_id, false).await.is_err() || oldest().is_none() {
                        break;
                    }
                }
                Some((true, _)) => tokio::time::sleep(std::time::Duration::from_millis(40)).await,
                Some((false, false)) => break,
                Some((false, true)) => {
                    let before = oldest();
                    pages += 1;
                    if self.load_messages(key, server_id, channel_id, true).await.is_err() || oldest() == before {
                        break;
                    }
                }
            }
        }
        found()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn queries_read_as_on_the_web() {
        let parsed = parse_query("from:mika in:general has:link cake ");
        assert_eq!(parsed.text, "cake ");
        assert_eq!(
            parsed.filters.iter().map(|f| (f.key, f.value.as_str())).collect::<Vec<_>>(),
            vec![(FilterKey::From, "mika"), (FilterKey::In, "general"), (FilterKey::Has, "link")]
        );
        // A key with nothing after it, or one that isn't a filter, is a word.
        assert_eq!(parse_query("from: http://x").text, "from: http://x");
        assert_eq!(parse_query("FROM:Mika").filters[0].key, FilterKey::From);
        assert_eq!(token_at("from:mi cake", 4), 0..7);
        assert_eq!(token_at("a b", 2), 2..3);
        assert_eq!(token_at("a ", 2), 2..2);
        assert_eq!(replace_token("cake from:mi", 12, "from:mika"), ("cake from:mika ".into(), 15));
        assert_eq!(has_value("Images"), Some(pb::SearchHas::Picture));
        assert_eq!(has_value("nope"), None);
    }

    #[test]
    fn dates_narrow_the_time_as_discord_reads_them() {
        let today = day(2026, 10, 4);
        assert_eq!(parse_day("yesterday", today), Some(day(2026, 10, 3)));
        assert_eq!(parse_day("2026-2-30", today), None);
        assert_eq!(parse_day("26-10-01", today), None);
        let f = |key, value: &str| Filter { key, value: value.into() };
        assert_eq!(
            time_range(&[f(FilterKey::After, "2026-10-01"), f(FilterKey::Before, "today")], today),
            Ok((Some(day(2026, 10, 2)), Some(today)))
        );
        assert_eq!(time_range(&[f(FilterKey::During, "today")], today), Ok((Some(today), Some(day(2026, 10, 5)))));
        assert!(time_range(&[f(FilterKey::Before, "soon")], today).is_err());
    }

    #[test]
    fn highlights_land_on_the_right_bytes() {
        let r = |start, end| pb::TextRange { start, end };
        let text = "héllo 🌸 cake";
        // UTF-16: h é l l o _ 🌸(2) _ c a k e
        let got = byte_ranges(text, &[r(0, 5), r(9, 13)]);
        assert_eq!(got.iter().map(|g| &text[g.clone()]).collect::<Vec<_>>(), vec!["héllo", "cake"]);
        // Out of order, splitting the flower, or past the end: left out.
        assert!(byte_ranges(text, &[r(9, 13), r(0, 5)]).len() == 1);
        assert!(byte_ranges(text, &[r(7, 8)]).is_empty());
        assert!(byte_ranges(text, &[r(10, 40)]).is_empty());
    }
}
