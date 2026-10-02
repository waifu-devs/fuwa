//! Text helpers: Markdown as the web app shows it, and times as people say them.

use chrono::{DateTime, Local, TimeZone as _};

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

/// "Today at 14:03", "Yesterday at 09:12", or the date.
pub fn when(ms: i64) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let now = Local::now();
    let days = now.date_naive().signed_duration_since(at.date_naive()).num_days();
    match days {
        0 => format!("Today at {}", at.format("%H:%M")),
        1 => format!("Yesterday at {}", at.format("%H:%M")),
        _ => at.format("%Y-%m-%d %H:%M").to_string(),
    }
}

/// Just the time, for the side of a follow-up message.
pub fn clock(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|at: DateTime<Local>| at.format("%H:%M").to_string())
        .unwrap_or_default()
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
}
