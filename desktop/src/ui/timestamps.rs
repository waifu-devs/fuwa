//! Timestamps in messages, as the web app has them: `<t:1759594800:R>` reads
//! "in 3 hours" for whoever sees it, on their own clock, and the composer's
//! timestamp button (or Alt+Shift+T) picks one and puts its token at the caret.

use std::time::Duration;

use chrono::{Datelike as _, Local, NaiveDate, NaiveTime, TimeZone as _, Timelike as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::core::dms::now_ms;
use crate::core::timestamps::{self, Style};
use crate::ui::app::FuwaApp;
use crate::ui::chat::Row;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{card, icon, icon_button, pal, primary_button};

/// Where a timestamp's node points, in the Markdown handed to the text view.
pub const SCHEME: &str = "fuwa-time:";

// ───────────────────────── In messages ─────────────────────────

/// Turns timestamp tokens into nodes the [`Plugin`] draws, leaving code and
/// links alone, as the web does. (Left as typed, Markdown would read the
/// token as HTML and drop it.)
pub fn timestamp_nodes(source: &str) -> String {
    if !source.contains("<t:") {
        return source.to_owned();
    }
    let mut out = String::with_capacity(source.len() + 16);
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
        let (mut in_code, mut bracket, mut paren) = (false, 0usize, 0usize);
        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            let c = rest.chars().next().unwrap_or_default();
            if !in_code
                && bracket == 0
                && paren == 0
                && let Some((seconds, style, len)) = timestamps::token_at(rest)
            {
                out.push_str(&format!("![time]({SCHEME}{seconds}:{})", style.letter()));
                i += len;
                continue;
            }
            match c {
                '`' => in_code = !in_code,
                '\\' if !in_code => {
                    // An escaped character goes along as it is.
                    out.push('\\');
                    i += 1;
                    if let Some(next) = line[i..].chars().next() {
                        out.push(next);
                        i += next.len_utf8();
                    }
                    continue;
                }
                '[' if !in_code => bracket += 1,
                ']' if !in_code => bracket = bracket.saturating_sub(1),
                '(' if !in_code && line[..i].ends_with(']') => paren += 1,
                ')' if !in_code && paren > 0 => paren -= 1,
                _ => {}
            }
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

struct Time {
    seconds: i64,
    style: Style,
}

/// Draws timestamp nodes as small chips, the whole date showing on hover.
pub struct Plugin;

impl gpui_kit::component::text::MarkdownPlugin for Plugin {
    fn name(&self) -> &str {
        "fuwa-time"
    }

    fn parse(
        &self,
        node: &gpui_kit::component::text::markdown_ast::Node,
        _: &gpui_kit::component::text::MarkdownParseContext<'_>,
    ) -> Option<gpui_kit::component::text::MarkdownNode> {
        let gpui_kit::component::text::markdown_ast::Node::Image(image) = node else { return None };
        let (seconds, letter) = image.url.strip_prefix(SCHEME)?.split_once(':')?;
        let style = Style::of(letter.chars().next()?)?;
        let seconds: i64 = seconds.parse().ok()?;
        let text = timestamps::token(seconds, style);
        Some(
            gpui_kit::component::text::MarkdownNode::new("fuwa-time", Time { seconds, style })
                .text(text.clone())
                .markdown(text),
        )
    }

    fn render_inline(
        &self,
        node: &gpui_kit::component::text::MarkdownNode,
        _: &gpui_kit::component::text::InlineRenderContext,
        _: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> Option<gpui_kit::component::text::InlineElement> {
        let time = node.data::<Time>()?;
        let p = pal(cx);
        let full = timestamps::full(time.seconds);
        Some(gpui_kit::component::text::InlineElement::new(
            div()
                .id(SharedString::from(format!("time|{}|{}", time.seconds, time.style.letter())))
                .mx(px(1.0))
                .px(px(4.0))
                .rounded(corner(6.0))
                .bg(alpha(p.foreground, 0.08))
                .font_weight(FontWeight::MEDIUM)
                .hover({
                    let (bg, fg) = (alpha(p.primary, 0.15), p.primary);
                    move |s| s.bg(bg).text_color(fg)
                })
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx))
                .child(timestamps::format(time.seconds, time.style, now_ms())),
        ))
    }
}

// ───────────────────────── The picker ─────────────────────────

/// The timestamp picker, while it's open.
pub struct TimePicker {
    date: Entity<InputState>,
    time: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// The moment the two fields name, on this computer's clock; None while one isn't a date or a time.
fn from_fields(date: &str, time: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let time = NaiveTime::parse_from_str(time.trim(), "%H:%M").ok()?;
    // A time the clocks skip (moving forward an hour) has no moment; one they repeat takes the first.
    Local.from_local_datetime(&date.and_time(time)).earliest().map(|at| at.timestamp())
}

/// One-tap times: in an hour, tonight, tomorrow morning, in a week (those still to come).
fn quick_picks(now_ms: i64) -> Vec<(&'static str, i64)> {
    let now = Local.timestamp_millis_opt(now_ms).single().unwrap_or_else(Local::now);
    let at = |days: i64, hour: u32, minute: u32| {
        let day = now.date_naive() + chrono::Days::new(days.max(0) as u64);
        Local
            .from_local_datetime(&day.and_hms_opt(hour, minute, 0).unwrap_or_default())
            .earliest()
            .map(|t| t.timestamp())
    };
    let soon = now + chrono::Duration::hours(1);
    let week = now + chrono::Duration::days(7);
    [
        ("In an hour", Some(soon.timestamp() - i64::from(soon.second()))),
        ("Tonight", at(0, 20, 0)),
        ("Tomorrow morning", at(1, 9, 0)),
        ("In a week", Some(week.timestamp() - i64::from(week.second()))),
    ]
    .into_iter()
    .filter_map(|(label, at)| Some((label, at?)))
    .filter(|(_, at)| *at * 1000 > now_ms)
    .collect()
}

impl FuwaApp {
    pub(crate) fn timestamp_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.time_picker.is_some();
        icon_button("time-open", "calendar-clock", p)
            .size(px(36.0))
            .when(open, |el| el.bg(alpha(p.primary, 0.12)).text_color(p.primary))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Insert a timestamp").build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.time_picker.is_some() {
                    this.close_time_picker(window, cx);
                } else {
                    this.open_time_picker(window, cx);
                }
            }))
            .into_any_element()
    }

    /// Opens the picker at the top of the next hour.
    pub(crate) fn open_time_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.target().is_none() || self.recording_here() {
            return;
        }
        self.emoji_open = false;
        self.picker = None;
        let next = Local::now() + chrono::Duration::hours(1);
        let next = next.with_minute(0).and_then(|t| t.with_second(0)).unwrap_or(next);
        let date_value = format!("{:04}-{:02}-{:02}", next.year(), next.month(), next.day());
        let time_value = format!("{:02}:00", next.hour());
        let date = cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD").default_value(date_value));
        let time = cx.new(|cx| InputState::new(window, cx).placeholder("HH:MM").default_value(time_value));
        let subs = [&date, &time]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.insert_timestamp(window, cx),
                    _ => {}
                })
            })
            .collect();
        date.update(cx, |s, cx| s.focus(window, cx));
        self.time_picker = Some(TimePicker { date, time, _subs: subs });
        self.time_tick(cx);
        cx.notify();
    }

    /// True when it was open.
    pub(crate) fn close_time_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.time_picker.take().is_none() {
            return false;
        }
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
        true
    }

    fn picked(&self, cx: &Context<Self>) -> Option<i64> {
        let picker = self.time_picker.as_ref()?;
        from_fields(&picker.date.read(cx).value(), &picker.time.read(cx).value())
    }

    fn set_picked(&mut self, seconds: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.time_picker else { return };
        let Some(at) = Local.timestamp_opt(seconds, 0).single() else { return };
        let (date, time) = (at.format("%Y-%m-%d").to_string(), at.format("%H:%M").to_string());
        picker.date.update(cx, |s, cx| s.set_value(date, window, cx));
        picker.time.update(cx, |s, cx| s.set_value(time, window, cx));
        cx.notify();
    }

    /// Puts the picked moment's token at the caret, in the style picked.
    fn insert_timestamp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(seconds) = self.picked(cx) else { return };
        let token = timestamps::token(seconds, self.time_style);
        self.time_picker = None;
        self.composer.update(cx, |state, cx| {
            state.focus(window, cx);
            state.replace(format!("{token} "), window, cx);
        });
        crate::core::reports::used("message.timestamp");
        cx.notify();
    }

    pub(crate) fn time_picker_panel(&mut self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.time_picker.as_ref()?;
        let now = now_ms();
        let picked = self.picked(cx);
        let chosen = self.time_style;

        let mut quick = div().flex().flex_wrap().gap(px(6.0));
        for (label, at) in quick_picks(now) {
            let lit = picked == Some(at);
            quick = quick.child(
                div()
                    .id(SharedString::from(format!("time-quick|{label}")))
                    .px(px(10.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .bg(if lit { alpha(p.primary, 0.18) } else { alpha(p.foreground, 0.06) })
                    .text_color(if lit { p.primary } else { p.foreground })
                    .hover(|s| s.bg(alpha(p.primary, 0.12)))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_picked(at, window, cx)))
                    .child(label),
            );
        }

        let field = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(label))
                .child(Input::new(input))
        };
        let fields = div().flex().gap(px(8.0)).child(field("DATE", &picker.date)).child(field("TIME", &picker.time));

        let mut styles = div().flex().flex_col().gap(px(2.0));
        for style in Style::ALL {
            let lit = style == chosen;
            styles = styles.child(
                div()
                    .id(SharedString::from(format!("time-style|{}", style.letter())))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .h(px(32.0))
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .when(lit, |el| el.bg(alpha(p.primary, 0.12)))
                    .hover(|s| s.bg(alpha(p.primary, 0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.time_style = style;
                        cx.notify();
                    }))
                    .child(div().w(px(130.0)).flex_none().text_xs().text_color(p.muted_foreground).child(style.name()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(if lit { FontWeight::BOLD } else { FontWeight::NORMAL })
                            .child(picked.map(|s| timestamps::format(s, style, now)).unwrap_or_else(|| "—".into())),
                    )
                    .when(lit, |el| el.child(icon("check").size(px(15.0)).text_color(p.primary))),
            );
        }

        let footer = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .pt(px(4.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(if picked.is_some() { p.muted_foreground } else { p.destructive })
                    .child(match picked {
                        Some(s) => timestamps::token(s, chosen),
                        None => "Write the date as 2026-10-04 and the time as 21:30.".into(),
                    }),
            )
            .child(
                primary_button("time-insert", "Insert", p)
                    .h(px(34.0))
                    .text_sm()
                    .when(picked.is_none(), |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, window, cx| this.insert_timestamp(window, cx))),
            );

        let body = card(p)
            .w(px(400.0))
            .rounded(corner(18.0))
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon("calendar-clock").size(px(18.0)).text_color(p.primary))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Insert a timestamp"))
                    .child(
                        div().ml_auto().text_xs().text_color(p.muted_foreground).child("On each reader's own clock"),
                    ),
            )
            .child(quick)
            .child(fields)
            .child(styles)
            .child(footer);
        Some(
            div()
                .id("time-panel")
                .absolute()
                .right(px(20.0))
                .bottom(gpui_kit::relative(1.0))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.close_time_picker(window, cx);
                }))
                .child(motion::rise(body.mb(px(-8.0)), "time-panel-rise", Duration::ZERO, 12.0))
                .into_any_element(),
        )
    }

    /// Redraws while relative timestamps are on screen (or the picker's
    /// previews are), as often as the soonest of them changes.
    pub(crate) fn time_tick(&mut self, cx: &mut Context<Self>) {
        if self.time_ticking {
            return;
        }
        let now = now_ms();
        let mut wait = self.time_picker.as_ref().map(|_| 1000);
        for row in self.rows.iter() {
            let Row::Msg(m) = row else { continue };
            for at in timestamps::relative_in(&m.content) {
                let every = timestamps::refresh_every_ms(at, now);
                wait = Some(wait.map_or(every, |w: i64| w.min(every)));
            }
        }
        let Some(wait) = wait else { return };
        self.time_ticking = true;
        // On the edge of the second (or minute), so counts step in time with the clock.
        let wait = wait - now.rem_euclid(wait) + 5;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(wait as u64)).await;
            let _ = this.update(cx, |this, cx| {
                this.time_ticking = false;
                cx.notify();
                this.time_tick(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_become_nodes_outside_code_and_links() {
        assert_eq!(timestamp_nodes("at <t:5:R>!"), "at ![time](fuwa-time:5:R)!");
        assert_eq!(timestamp_nodes("<t:5> and <t:6:t>"), "![time](fuwa-time:5:f) and ![time](fuwa-time:6:t)");
        assert_eq!(timestamp_nodes("`<t:5:R>`"), "`<t:5:R>`");
        assert_eq!(timestamp_nodes("[<t:5:R>](https://x)"), "[<t:5:R>](https://x)");
        assert_eq!(timestamp_nodes("```\n<t:5:R>\n```"), "```\n<t:5:R>\n```");
        assert_eq!(timestamp_nodes("not <t:5:x> é"), "not <t:5:x> é");
        assert_eq!(timestamp_nodes("\\<t:5:R>"), "\\<t:5:R>");
    }

    #[test]
    fn fields_name_a_moment_on_this_clock() {
        let at = from_fields("2026-10-04", "21:30").unwrap();
        let back = Local.timestamp_opt(at, 0).single().unwrap();
        assert_eq!(back.format("%Y-%m-%d %H:%M").to_string(), "2026-10-04 21:30");
        assert_eq!(from_fields("2026-13-04", "21:30"), None);
        assert_eq!(from_fields("2026-10-04", ""), None);
        assert!(quick_picks(now_ms()).iter().all(|(_, at)| *at * 1000 > now_ms()));
    }
}
