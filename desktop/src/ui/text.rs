//! Text helpers: Markdown as the web app shows it, times as people say them,
//! and lines with letter spacing.

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

/// A single newline is a line break, as the web's `remark-breaks` reads
/// Markdown: lines that run on get Markdown's hard break (two spaces). Code
/// blocks are left alone.
pub fn hard_breaks(source: &str) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out = String::with_capacity(source.len() + lines.len() * 2);
    let mut fenced = false;
    for (n, line) in lines.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        out.push_str(line);
        let trimmed = line.trim_start();
        let fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if fence {
            fenced = !fenced;
            continue;
        }
        let next = lines.get(n + 1).map(|l| l.trim_start());
        let runs_on = next.is_some_and(|next| !next.is_empty() && !next.starts_with("```") && !next.starts_with("~~~"));
        if !fenced && runs_on && !line.trim().is_empty() && !line.ends_with("  ") && !line.ends_with('\\') {
            out.push_str("  ");
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
pub fn twelve_hours() -> bool {
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

/// "Today", "Yesterday", "Monday, June 3", or with the year when it's not
/// this year: a day divider's words, as the web's `formatDay`.
pub fn day(ms: i64) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let now = Local::now();
    match now.date_naive().signed_duration_since(at.date_naive()).num_days() {
        0 => crate::core::i18n::t("common.time.today"),
        1 => crate::core::i18n::t("common.time.yesterday"),
        _ if at.year() == now.year() => at.format("%A, %B %-d").to_string(),
        _ => at.format("%B %-d, %Y").to_string(),
    }
}

/// Whether two moments fall on the same local day.
pub fn same_day(a: i64, b: i64) -> bool {
    let day = |ms: i64| Local.timestamp_millis_opt(ms).single().map(|at| at.date_naive());
    day(a) == day(b)
}

/// "just now", "5 minutes ago", "3 days ago": how long ago, rounded down to
/// its largest unit, as the web's `ago`.
pub fn ago(ms: i64, now: i64) -> String {
    let minutes = (now - ms).max(0) / 60_000;
    let (n, unit) = match minutes {
        0 => return crate::core::i18n::t("common.time.justNow"),
        m if m < 60 => (m, "minute"),
        m if m < 60 * 24 => (m / 60, "hour"),
        m if m < 60 * 24 * 30 => (m / (60 * 24), "day"),
        m if m < 60 * 24 * 365 => (m / (60 * 24 * 30), "month"),
        m => (m / (60 * 24 * 365), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// "Thursday, October 9, 2026 at 3:04 PM": the whole moment, for the tooltip
/// on a message's time (the web's `formatFull`, its `title`).
pub fn full(ms: i64) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let (day, time) = (at.format("%A, %B %-d, %Y").to_string(), time_of(&at));
    crate::core::i18n::t_with(
        "common.time.dayTime",
        &[("day", crate::core::i18n::Arg::Str(&day)), ("time", crate::core::i18n::Arg::Str(&time))],
    )
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

/// Tailwind's letter spacing, in ems: `tracking-tight`, `-wide`, `-wider`, `-widest`.
pub const TIGHT: f32 = -0.025;
pub const WIDE: f32 = 0.025;
pub const WIDER: f32 = 0.05;
pub const WIDEST: f32 = 0.1;

/// One line of text with letter spacing (CSS `letter-spacing`), which GPUI
/// doesn't have: the line is shaped as usual, then every letter moves over by
/// `em` times the font size for each letter before it. It takes its font, size
/// and color from the element it's in. It's one line, which ends in an ellipsis
/// when squeezed, unless it `wraps`.
pub fn tracked(text: impl Into<gpui_kit::SharedString>, em: f32) -> Tracked {
    Tracked { text: text.into(), em, colors: None, wraps: false }
}

pub struct Tracked {
    text: gpui_kit::SharedString,
    em: f32,
    colors: Option<Vec<gpui_kit::Hsla>>,
    wraps: bool,
}

impl Tracked {
    /// Breaks into lines between words (or inside a word too long for a line,
    /// like the web's `break-words`) instead of ending in an ellipsis.
    pub fn wraps(mut self) -> Self {
        self.wraps = true;
        self
    }

    /// A color for each letter, in order (letters past the end keep the text's).
    pub fn letter_colors(mut self, colors: Vec<gpui_kit::Hsla>) -> Self {
        self.colors = Some(colors);
        self
    }

    /// Shapes `text` (all of it, or the start of it) with the spacing.
    fn shape(&self, text: &str, style: &gpui_kit::TextStyle, window: &mut gpui_kit::Window) -> gpui_kit::ShapedLine {
        let font_size = style.font_size.to_pixels(window.rem_size());
        let runs: Vec<gpui_kit::TextRun> = match &self.colors {
            Some(colors) => text
                .chars()
                .enumerate()
                .map(|(n, c)| {
                    let mut run = style.to_run(c.len_utf8());
                    if let Some(color) = colors.get(n) {
                        run.color = *color;
                    }
                    run
                })
                .collect(),
            None => vec![style.to_run(text.len())],
        };
        let mut line = window.text_system().shape_line(text.to_owned().into(), font_size, &runs, None);
        *line = std::sync::Arc::new(spread(&line, text, font_size * self.em));
        line
    }
}

/// Moves each glyph over by `spacing` for every letter before it, and widens
/// the line by the same after each letter, as browsers do.
fn spread(layout: &gpui_kit::LineLayout, text: &str, spacing: gpui_kit::Pixels) -> gpui_kit::LineLayout {
    let starts: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    let before = |index: usize| starts.partition_point(|&at| at < index) as f32;
    let runs = layout
        .runs
        .iter()
        .map(|run| gpui_kit::ShapedRun {
            font_id: run.font_id,
            glyphs: run
                .glyphs
                .iter()
                .map(|g| {
                    let mut g = g.clone();
                    g.position.x += spacing * before(g.index);
                    g
                })
                .collect(),
        })
        .collect();
    gpui_kit::LineLayout {
        font_size: layout.font_size,
        width: layout.width + spacing * starts.len() as f32,
        ascent: layout.ascent,
        descent: layout.descent,
        runs,
        len: layout.len,
    }
}

impl gpui_kit::IntoElement for Tracked {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Where each letter starts on the shaped line, and where the line ends.
fn edges(line: &gpui_kit::LineLayout, text: &str) -> Vec<(usize, f32)> {
    let mut at: Vec<(usize, f32)> =
        line.runs.iter().flat_map(|run| run.glyphs.iter().map(|g| (g.index, f32::from(g.position.x)))).collect();
    at.sort_by_key(|(index, _)| *index);
    let mut out: Vec<(usize, f32)> = Vec::new();
    for (start, _) in text.char_indices() {
        // A letter without a glyph of its own (in a ligature) starts with the one before it.
        let x = at
            .iter()
            .find(|(index, _)| *index == start)
            .map(|(_, x)| *x)
            .unwrap_or_else(|| out.last().map(|(_, x)| *x).unwrap_or(0.0));
        out.push((start, x));
    }
    out.push((text.len(), f32::from(line.width)));
    out
}

/// The lines `text` breaks into at `width`, as byte ranges: between words
/// where it can, inside a word too long for a line. Spaces at a break go.
fn wrap_lines(text: &str, edges: &[(usize, f32)], width: f32) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut space: Option<usize> = None;
    for k in 0..edges.len().saturating_sub(1) {
        if text[edges[k].0..edges[k + 1].0].trim().is_empty() {
            space = Some(k);
            continue;
        }
        while k > start && edges[k + 1].1 - edges[start].1 > width + 0.5 {
            match space.filter(|&s| s > start) {
                Some(s) => {
                    lines.push(edges[start].0..edges[s].0);
                    start = s + 1;
                }
                None => {
                    lines.push(edges[start].0..edges[k].0);
                    start = k;
                }
            }
            space = None;
        }
    }
    lines.push(edges[start].0..text.len());
    lines
}

/// The widest of `lines`.
fn widest(text: &str, edges: &[(usize, f32)], lines: &[std::ops::Range<usize>]) -> f32 {
    let x = |at: usize| edges.iter().find(|(start, _)| *start == at).map(|(_, x)| *x).unwrap_or(0.0);
    lines.iter().map(|r| x(r.start + text[r.clone()].trim_end().len()) - x(r.start)).fold(0.0, f32::max)
}

pub struct TrackedLayout {
    lines: Vec<gpui_kit::ShapedLine>,
    line_height: gpui_kit::Pixels,
    style: gpui_kit::TextStyle,
}

impl gpui_kit::Element for Tracked {
    type RequestLayoutState = TrackedLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui_kit::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> (gpui_kit::LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(style.line_height.to_pixels(font_size.into(), window.rem_size()));
        let line = self.shape(&self.text.clone(), &style, window);
        let id = if self.wraps {
            // As many lines as the width it's given takes.
            let text = self.text.clone();
            let edges = edges(&line, &text);
            let full = f32::from(line.width);
            window.request_measured_layout(gpui_kit::Style::default(), move |known, available, _, _| {
                let width = match (known.width, available.width) {
                    (Some(w), _) | (None, gpui_kit::AvailableSpace::Definite(w)) => f32::from(w),
                    (None, gpui_kit::AvailableSpace::MinContent) => 0.0,
                    (None, gpui_kit::AvailableSpace::MaxContent) => full,
                };
                let lines = wrap_lines(&text, &edges, width);
                gpui_kit::size(gpui_kit::px(widest(&text, &edges, &lines)), line_height * lines.len() as f32)
            })
        } else {
            // As wide as the line, but it may be squeezed, and then ends in an ellipsis.
            let mut layout =
                gpui_kit::Style { size: gpui_kit::size(line.width.into(), line_height.into()), ..Default::default() };
            layout.min_size.width = gpui_kit::px(0.0).into();
            window.request_layout(layout, [], cx)
        };
        (id, TrackedLayout { lines: vec![line], line_height, style })
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut gpui_kit::Window,
        _cx: &mut gpui_kit::App,
    ) {
        let full = state.lines[0].width;
        if full <= bounds.size.width + gpui_kit::px(1.0) {
            return;
        }
        let text = self.text.clone();
        if self.wraps {
            let edges = edges(&state.lines[0], &text);
            state.lines = wrap_lines(&text, &edges, f32::from(bounds.size.width))
                .into_iter()
                .map(|r| self.shape(text[r].trim_end(), &state.style, window))
                .collect();
            return;
        }
        for (at, _) in text.char_indices().rev() {
            let cut = format!("{}…", text[..at].trim_end());
            let line = self.shape(&cut, &state.style, window);
            if line.width <= bounds.size.width || at == 0 {
                state.lines = vec![line];
                return;
            }
        }
    }

    fn paint(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
        state: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) {
        let align = window.text_style().text_align;
        for (n, line) in state.lines.iter().enumerate() {
            let origin = bounds.origin + gpui_kit::point(gpui_kit::px(0.0), state.line_height * n as f32);
            let _ = line.paint(origin, state.line_height, align, Some(bounds.size.width), window, cx);
        }
    }
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
    fn letters_spread_by_the_letters_before_them() {
        use gpui_kit::{FontId, GlyphId, LineLayout, ShapedGlyph, ShapedRun, point, px};
        // "aé b": é is two bytes, so glyphs are found by letter, not by byte.
        let glyph = |index: usize, x: f32| ShapedGlyph {
            id: GlyphId(0),
            position: point(px(x), px(0.0)),
            index,
            is_emoji: false,
        };
        let layout = LineLayout {
            font_size: px(10.0),
            width: px(40.0),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                glyphs: vec![glyph(0, 0.0), glyph(1, 10.0), glyph(3, 20.0), glyph(4, 30.0)],
            }],
            len: 5,
            ..Default::default()
        };
        let wide = spread(&layout, "aé b", px(1.5));
        let xs: Vec<f32> = wide.runs[0].glyphs.iter().map(|g| f32::from(g.position.x)).collect();
        assert_eq!(xs, [0.0, 11.5, 23.0, 34.5]);
        assert_eq!(wide.width, px(46.0));
    }

    #[test]
    fn long_lines_break_between_words_then_inside_them() {
        let at = |text: &str| -> Vec<(usize, f32)> {
            let mut e: Vec<(usize, f32)> = text.char_indices().map(|(i, _)| (i, i as f32 * 10.0)).collect();
            e.push((text.len(), text.len() as f32 * 10.0));
            e
        };
        let text = "ab cd efghij";
        let lines = |w: f32| -> Vec<&str> { wrap_lines(text, &at(text), w).into_iter().map(|r| &text[r]).collect() };
        assert_eq!(lines(200.0), ["ab cd efghij"]);
        assert_eq!(lines(55.0), ["ab cd", "efghi", "j"]);
        assert_eq!(lines(30.0), ["ab", "cd", "efg", "hij"]);
        assert_eq!(widest(text, &at(text), &wrap_lines(text, &at(text), 55.0)), 50.0);
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
