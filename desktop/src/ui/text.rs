//! Text helpers: Markdown as the web app shows it, and times as people say them.

use chrono::{DateTime, Datelike as _, Local, TimeZone as _};

/// Pictures in messages show as links, as on the web, so nobody's address is
/// fetched by just opening a channel. Code is left alone.
pub fn images_as_links(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut fenced = false;
    for (n, line) in source.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced {
            out.push_str(line);
            continue;
        }
        let mut in_code = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '`' => {
                    in_code = !in_code;
                    out.push(c);
                }
                '\\' => {
                    out.push(c);
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                '!' if !in_code && chars.peek() == Some(&'[') => {}
                _ => out.push(c),
            }
        }
    }
    out
}

/// The clock setting (`Prefs::clock`), kept where formatting can read it: 0 auto, 1 12h, 2 24h.
static CLOCK: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_clock(clock: crate::core::config::Clock) {
    use crate::core::config::Clock;
    let n = match clock {
        Clock::Auto => 0,
        Clock::H12 => 1,
        Clock::H24 => 2,
    };
    CLOCK.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// Whether times read on a 12 hour clock: as set, or the language's own
/// (English, Korean and Hindi count 12 hours, as their `Intl` formats do).
fn twelve_hours() -> bool {
    match CLOCK.load(std::sync::atomic::Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let (code, _) = crate::core::i18n::current();
            let lang = code.split(['-', '_']).next().unwrap_or("");
            matches!(lang, "en" | "ko" | "hi") && !matches!(code.as_str(), "en-GB")
        }
    }
}

fn time_of(at: &DateTime<Local>) -> String {
    if twelve_hours() { at.format("%-I:%M %p").to_string() } else { at.format("%H:%M").to_string() }
}

/// "Today at 3:04 PM", "Yesterday at 9:12 AM", or the day and time, as the web's `formatStamp`.
pub fn when(ms: i64) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let now = Local::now();
    let days = now.date_naive().signed_duration_since(at.date_naive()).num_days();
    let time = time_of(&at);
    let arg = crate::core::i18n::Arg::Str(&time);
    match days {
        0 => crate::core::i18n::t_with("common.time.todayAt", &[("time", arg)]),
        1 => crate::core::i18n::t_with("common.time.yesterdayAt", &[("time", arg)]),
        _ => {
            let day = if at.year() == now.year() {
                at.format("%A, %B %-d").to_string()
            } else {
                at.format("%B %-d, %Y").to_string()
            };
            let d = crate::core::i18n::Arg::Str(&day);
            crate::core::i18n::t_with(
                "common.time.dayTime",
                &[("day", d), ("time", crate::core::i18n::Arg::Str(&time))],
            )
        }
    }
}

/// Just the time, for the side of a follow-up message.
pub fn clock(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|at: DateTime<Local>| time_of(&at)).unwrap_or_default()
}

pub fn ms_of(t: Option<&prost_types::Timestamp>) -> i64 {
    t.map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000).unwrap_or(0)
}

/// A 60-digit safety number as twelve groups of five, three rows of four.
pub fn safety_rows(number: &str) -> Vec<String> {
    let digits: Vec<char> = number.chars().filter(char::is_ascii_digit).collect();
    let groups: Vec<String> = digits.chunks(5).map(|c| c.iter().collect()).collect();
    groups.chunks(4).map(|row| row.join("  ")).collect()
}

/// Whether a link from someone else's words may be opened: only web pages
/// and mail. Anything else (`file:`, `smb:`, `ms-*:` and other handlers the
/// system knows) could run or reach something on this computer or network.
pub fn safe_link(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else { return false };
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => rest.starts_with("//") && rest.len() > 2,
        "mailto" => !rest.is_empty(),
        _ => false,
    }
}

/// Opens a link someone wrote, when it's safe to.
pub fn open_link(url: &str, cx: &mut gpui_kit::App) {
    if safe_link(url) {
        cx.open_url(url);
    } else {
        tracing::warn!("not opening a link that isn't a web page or mail address");
    }
}

/// Markdown whose links open only when they're safe, for anything people wrote.
pub fn markdown(
    id: impl Into<gpui_kit::ElementId>,
    source: impl Into<gpui_kit::SharedString>,
) -> gpui_kit::component::text::TextView {
    gpui_kit::component::text::TextView::markdown(id, source).on_link_click(|url, _, _, cx| open_link(url, cx))
}

/// A line from the translations with some of its placeholders filled in
/// bold (the web's `<T values={{ x: <b>…</b> }}>`), such as the composer's hint.
pub fn hint_line(template: &str, values: &[(&str, &str)], _p: &crate::ui::theme::Palette) -> gpui_kit::StyledText {
    let mut text = String::new();
    let mut bold = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else { break };
        let name = &rest[open + 1..open + close];
        match values.iter().find(|(k, _)| *k == name) {
            Some((_, value)) => {
                let start = text.len();
                text.push_str(value);
                bold.push((
                    start..text.len(),
                    gpui_kit::HighlightStyle { font_weight: Some(gpui_kit::FontWeight::BOLD), ..Default::default() },
                ));
            }
            None => text.push_str(&rest[open..open + close + 1]),
        }
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    gpui_kit::StyledText::new(text).with_highlights(bold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_become_links_but_code_stays() {
        assert_eq!(images_as_links("look ![cat](https://x/cat.png)"), "look [cat](https://x/cat.png)");
        assert_eq!(images_as_links("`![a](b)` and ![c](d)"), "`![a](b)` and [c](d)");
        assert_eq!(images_as_links("```\n![a](b)\n```\n![c](d)"), "```\n![a](b)\n```\n[c](d)");
        assert_eq!(images_as_links("wow! [link](x)"), "wow! [link](x)");
        assert_eq!(images_as_links(r"\![not](img)"), r"\![not](img)");
    }

    #[test]
    fn safety_numbers_read_in_groups() {
        let n: String = (0..60).map(|i| char::from(b'0' + (i % 10) as u8)).collect();
        let rows = safety_rows(&n);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], "01234  56789  01234  56789");
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(safe_link("https://fuwa.chat/x"));
        assert!(safe_link("HTTP://example.com"));
        assert!(safe_link("mailto:hi@example.com"));
        for bad in
            ["file:///etc/passwd", "smb://host/share", "ms-settings:", "javascript:alert(1)", "http:", "notalink"]
        {
            assert!(!safe_link(bad), "{bad}");
        }
    }
}
