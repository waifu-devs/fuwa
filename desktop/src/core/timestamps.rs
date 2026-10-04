//! Timestamps in messages, written the way Discord (and the web app) writes
//! them: `<t:SECONDS>` or `<t:SECONDS:STYLE>`, where SECONDS is a Unix time.
//! Everyone reads them in their own time zone; nothing about this computer's
//! time zone ever leaves it.

use chrono::{DateTime, Local, TimeZone as _};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    ShortTime,
    LongTime,
    ShortDate,
    LongDate,
    DateTime,
    DayDateTime,
    Relative,
}

impl Style {
    /// Every style, in the order the picker lists them.
    pub const ALL: [Style; 7] = [
        Style::ShortTime,
        Style::LongTime,
        Style::ShortDate,
        Style::LongDate,
        Style::DateTime,
        Style::DayDateTime,
        Style::Relative,
    ];

    pub fn letter(self) -> char {
        match self {
            Style::ShortTime => 't',
            Style::LongTime => 'T',
            Style::ShortDate => 'd',
            Style::LongDate => 'D',
            Style::DateTime => 'f',
            Style::DayDateTime => 'F',
            Style::Relative => 'R',
        }
    }

    pub fn of(letter: char) -> Option<Self> {
        Style::ALL.into_iter().find(|s| s.letter() == letter)
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::ShortTime => "Short time",
            Style::LongTime => "Long time",
            Style::ShortDate => "Short date",
            Style::LongDate => "Long date",
            Style::DateTime => "Date and time",
            Style::DayDateTime => "Day, date and time",
            Style::Relative => "Relative",
        }
    }
}

/// What `<t:SECONDS>` with no style shows, as in Discord.
pub const DEFAULT: Style = Style::DateTime;

/// The furthest the web app reads either way, in seconds.
const LIMIT: i64 = 8_640_000_000_000;

/// A token at the start of `text`: its time in seconds, its style, and how long the token is.
pub fn token_at(text: &str) -> Option<(i64, Style, usize)> {
    let rest = text.strip_prefix("<t:")?;
    let digits_from = usize::from(rest.starts_with('-'));
    let digits = rest[digits_from..].bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=14).contains(&digits) {
        return None;
    }
    let number_end = digits_from + digits;
    let seconds: i64 = rest[..number_end].parse().ok()?;
    if seconds.abs() > LIMIT {
        return None;
    }
    let after = &rest[number_end..];
    let (style, used) = match after.as_bytes() {
        [b'>', ..] => (DEFAULT, 1),
        [b':', letter, b'>', ..] => (Style::of(char::from(*letter))?, 3),
        _ => return None,
    };
    // It reads only when this computer's calendar reaches it.
    local(seconds * 1000)?;
    Some((seconds, style, 3 + number_end + used))
}

/// The token for a moment, ready to send; the default style is written bare, as Discord's picker leaves it.
pub fn token(seconds: i64, style: Style) -> String {
    if style == DEFAULT { format!("<t:{seconds}>") } else { format!("<t:{seconds}:{}>", style.letter()) }
}

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// A timestamp as its style shows it, on this computer's clock.
pub fn format(seconds: i64, style: Style, now_ms: i64) -> String {
    let Some(at) = local(seconds.saturating_mul(1000)) else { return token(seconds, style) };
    match style {
        Style::ShortTime => at.format("%H:%M").to_string(),
        Style::LongTime => at.format("%H:%M:%S").to_string(),
        Style::ShortDate => at.format("%Y-%m-%d").to_string(),
        Style::LongDate => at.format("%B %-d, %Y").to_string(),
        Style::DateTime => at.format("%B %-d, %Y %H:%M").to_string(),
        Style::DayDateTime => at.format("%A, %B %-d, %Y %H:%M").to_string(),
        Style::Relative => relative(seconds.saturating_mul(1000), now_ms),
    }
}

/// The whole date, for when the pointer rests on one.
pub fn full(seconds: i64) -> String {
    format(seconds, Style::DayDateTime, 0)
}

const SECOND: i64 = 1000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// The unit a gap reads in, with Discord's (and the web's) cut-offs.
fn unit_for(gap: i64) -> (&'static str, i64) {
    let abs = gap.saturating_abs();
    if abs < 45 * SECOND {
        ("second", SECOND)
    } else if abs < 45 * MINUTE {
        ("minute", MINUTE)
    } else if abs < 22 * HOUR {
        ("hour", HOUR)
    } else if abs < 26 * DAY {
        ("day", DAY)
    } else if abs < 11 * MONTH {
        ("month", MONTH)
    } else {
        ("year", YEAR)
    }
}

/// "in 3 hours", "2 days ago", "tomorrow": how far a moment is from now.
pub fn relative(ms: i64, now_ms: i64) -> String {
    let gap = ms.saturating_sub(now_ms);
    let (unit, size) = unit_for(gap);
    let rounded = (gap as f64 / size as f64).round() as i64;
    // At least one of the unit, so seconds count down to "now" but nothing else does.
    let amount = if rounded == 0 && unit != "second" { gap.signum() } else { rounded };
    match (unit, amount) {
        ("second", 0) => "now".into(),
        ("day", 1) => "tomorrow".into(),
        ("day", -1) => "yesterday".into(),
        ("month" | "year", 1) => format!("next {unit}"),
        ("month" | "year", -1) => format!("last {unit}"),
        _ => {
            let n = amount.unsigned_abs();
            let plural = if n == 1 { "" } else { "s" };
            if amount > 0 { format!("in {n} {unit}{plural}") } else { format!("{n} {unit}{plural} ago") }
        }
    }
}

/// How soon a relative timestamp could read differently: each second while
/// it counts seconds, else each minute.
pub fn refresh_every_ms(ms: i64, now_ms: i64) -> i64 {
    if unit_for(ms.saturating_sub(now_ms)).0 == "second" { SECOND } else { MINUTE }
}

/// The relative timestamps in some text, as Unix milliseconds (outside code, as they're drawn).
pub fn relative_in(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find("<t:") {
        at += found;
        match token_at(&text[at..]) {
            Some((seconds, Style::Relative, len)) => {
                out.push(seconds * 1000);
                at += len;
            }
            _ => at += 3,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_759_594_830;

    #[test]
    fn tokens_read_as_on_the_web() {
        assert_eq!(token_at("<t:1759594830:R> later"), Some((AT, Style::Relative, 16)));
        assert_eq!(token_at("<t:1759594830>"), Some((AT, DEFAULT, 14)));
        assert_eq!(token_at("<t:-5:t>"), Some((-5, Style::ShortTime, 8)));
        for bad in ["<t:12:x>", "<t:abc>", "<t:>", "<t:123", "<t:123456789012345>", "<t:1:R"] {
            assert_eq!(token_at(bad), None, "{bad}");
        }
        assert_eq!(token(AT, Style::Relative), "<t:1759594830:R>");
        assert_eq!(token(AT, Style::DateTime), "<t:1759594830>");
        assert_eq!(relative_in("a <t:5:R> b <t:6> c <t:7:R>"), vec![5000, 7000]);
    }

    #[test]
    fn relative_times_use_discords_cut_offs() {
        let now = AT * 1000;
        assert_eq!(relative(now + 5 * SECOND, now), "in 5 seconds");
        assert_eq!(relative(now, now), "now");
        assert_eq!(relative(now - SECOND, now), "1 second ago");
        assert_eq!(relative(now + 44 * MINUTE, now), "in 44 minutes");
        assert_eq!(relative(now + 3 * HOUR, now), "in 3 hours");
        assert_eq!(relative(now + DAY, now), "tomorrow");
        assert_eq!(relative(now - 2 * DAY, now), "2 days ago");
        assert_eq!(relative(now + 40 * DAY, now), "next month");
        assert_eq!(relative(now - 3 * YEAR, now), "3 years ago");
        assert_eq!(refresh_every_ms(now + 10 * SECOND, now), SECOND);
        assert_eq!(refresh_every_ms(now + HOUR, now), MINUTE);
    }

    #[test]
    fn every_style_formats() {
        for style in Style::ALL {
            assert!(!format(AT, style, AT * 1000).is_empty());
            assert_eq!(Style::of(style.letter()), Some(style));
        }
        assert_eq!(format(AT, Style::Relative, AT * 1000), "now");
    }
}
