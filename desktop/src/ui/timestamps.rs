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
use crate::core::i18n::t;
use crate::core::timestamps::{self, Style};
use crate::ui::app::FuwaApp;
use crate::ui::chat::Row;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, pal, primary_button};

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
        let full = timestamps::format_clock(time.seconds, Style::DayDateTime, 0, crate::ui::text::twelve_hours());
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
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(full.clone()).build(window, cx))
                .child(timestamps::format_clock(time.seconds, time.style, now_ms(), crate::ui::text::twelve_hours())),
        ))
    }
}

// ───────────────────────── The picker ─────────────────────────

/// The timestamp picker, while it's open.
pub struct TimePicker {
    date: Entity<InputState>,
    time: Entity<InputState>,
    /// Which field has the focus (0 the date, 1 the time), for its ring.
    focus: Option<u8>,
    _subs: Vec<Subscription>,
}

/// The moment the two fields name, on this computer's clock; None while one isn't a date or a time.
fn from_fields(date: &str, time: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let time = NaiveTime::parse_from_str(time.trim(), "%H:%M").ok()?;
    // A time the clocks skip (moving forward an hour) has no moment; one they repeat takes the first.
    Local.from_local_datetime(&date.and_time(time)).earliest().map(|at| at.timestamp())
}

/// A style's name, from the translations.
fn style_name(style: Style) -> &'static str {
    match style {
        Style::ShortTime => "chattools.timestamp.style.shortTime",
        Style::LongTime => "chattools.timestamp.style.longTime",
        Style::ShortDate => "chattools.timestamp.style.shortDate",
        Style::LongDate => "chattools.timestamp.style.longDate",
        Style::DateTime => "chattools.timestamp.style.dateTime",
        Style::DayDateTime => "chattools.timestamp.style.dayDateTime",
        Style::Relative => "chattools.timestamp.style.relative",
    }
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
        ("chattools.timestamp.inAnHour", Some(soon.timestamp() - i64::from(soon.second()))),
        ("chattools.timestamp.tonight", at(0, 20, 0)),
        ("chattools.timestamp.tomorrowMorning", at(1, 9, 0)),
        ("chattools.timestamp.inAWeek", Some(week.timestamp() - i64::from(week.second()))),
    ]
    .into_iter()
    .filter_map(|(label, at)| Some((label, at?)))
    .filter(|(_, at)| *at * 1000 > now_ms)
    .collect()
}

impl FuwaApp {
    pub(crate) fn timestamp_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.time_picker.is_some();
        crate::ui::widgets::tool_button("time-open", "calendar-clock", open, p)
            .hover(|s| s.scale(1.12).rotate(gpui_kit::radians(8f32.to_radians())))
            .active(|s| s.scale(0.85))
            .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chattools.timestamp.insert")).build(window, cx))
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
            .enumerate()
            .map(|(n, input)| {
                cx.subscribe_in(input, window, move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.insert_timestamp(window, cx),
                    InputEvent::Focus | InputEvent::Blur => {
                        if let Some(picker) = &mut this.time_picker {
                            let focused = matches!(event, InputEvent::Focus);
                            if focused {
                                picker.focus = Some(n as u8);
                            } else if picker.focus == Some(n as u8) {
                                picker.focus = None;
                            }
                        }
                        cx.notify()
                    }
                })
            })
            .collect();
        date.update(cx, |s, cx| s.focus(window, cx));
        self.time_picker = Some(TimePicker { date, time, focus: Some(0), _subs: subs });
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
        // A secure channel's thread has a box of its own.
        let input = if crate::ui::secure_threads::time_in_thread()
            && matches!(self.target(), Some(crate::ui::app::Target::Secure { .. }))
        {
            self.threads.reply.clone()
        } else {
            self.composer.clone()
        };
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.replace(format!("{token} "), window, cx);
        });
        crate::core::reports::used("message.timestamp");
        cx.notify();
    }

    /// The picker (the web's `TimestampPicker` panel), above the composer's
    /// timestamp button: the day and time with one-tap picks, how it should
    /// read, and the token that goes in.
    pub(crate) fn time_picker_panel(&mut self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.time_picker.as_ref()?;
        let now = now_ms();
        let picked = self.picked(cx);
        let chosen = self.time_style;

        let field = |input: &Entity<InputState>, focused: bool| {
            div()
                .h(px(36.0))
                .flex()
                .items_center()
                .rounded(radius_xl())
                // Opaque, so the focus ring (a shadow) stays outside it as on the web.
                .bg(crate::ui::theme::mix(p.card, p.muted, 0.6))
                .when(focused, |el| {
                    el.shadow(vec![gpui_kit::BoxShadow {
                        color: alpha(p.primary, 0.4),
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(2.0),
                        inset: false,
                    }])
                })
                .child(div().flex_1().min_w_0().child(Input::new(input).appearance(false).text_sm()))
        };
        let fields = div()
            .mt(px(10.0))
            .flex()
            .gap(px(8.0))
            .child(div().flex_1().min_w_0().child(field(&picker.date, picker.focus == Some(0))))
            .child(div().w(px(118.0)).flex_none().child(field(&picker.time, picker.focus == Some(1))));

        let mut quick = div().mt(px(8.0)).pb(px(2.0)).flex().gap(px(6.0)).overflow_hidden();
        for (n, (label, at)) in quick_picks(now).into_iter().enumerate() {
            let lit = picked == Some(at);
            let (muted, fg) = (p.muted, p.foreground);
            quick = quick.child(motion::rise(
                div()
                    .id(SharedString::from(format!("time-quick|{label}")))
                    .flex_none()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded_full()
                    .border_1()
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(lit, |el| {
                        el.border_color(alpha(p.primary, 0.5)).bg(alpha(p.primary, 0.15)).text_color(p.primary)
                    })
                    .when(!lit, |el| {
                        el.border_color(p.border)
                            .text_color(p.muted_foreground)
                            .hover(move |s| s.bg(muted).text_color(fg))
                    })
                    .active(|s| s.scale(0.92))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_picked(at, window, cx)))
                    .child(t(label)),
                SharedString::from(format!("time-quick-in|{n}")),
                Duration::from_millis(40 + 30 * n as u64),
                4.0,
            ));
        }

        let top = div()
            .border_b_1()
            .border_color(p.border)
            .px(px(12.0))
            .pt(px(12.0))
            .pb(px(10.0))
            .child(
                div()
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(t("chattools.timestamp.insert")),
            )
            .child(
                div()
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("chattools.timestamp.about")),
            )
            .child(fields)
            .child(quick);

        // The picked style's tint glides to it (the web's `layoutId`): every row is 48px.
        let place = Style::ALL.iter().position(|s| *s == chosen).unwrap_or(0) as f32;
        let tint = alpha(p.primary, 0.12);
        let mut styles = div().relative().p(px(6.0)).flex().flex_col().child(crate::ui::motion::springing(
            "time-style-glide",
            place,
            move |at| {
                div()
                    .absolute()
                    .left(px(6.0))
                    .right(px(6.0))
                    .top(px(6.0 + 48.0 * at))
                    .h(px(48.0))
                    .rounded(radius_xl())
                    .bg(tint)
                    .into_any_element()
            },
        ));
        for (n, style) in Style::ALL.into_iter().enumerate() {
            let lit = style == chosen;
            styles = styles.child(motion::rise(
                div()
                    .id(SharedString::from(format!("time-style|{}", style.letter())))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(radius_xl())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, e: &gpui_kit::ClickEvent, window, cx| {
                        this.time_style = style;
                        // A double click puts it in straight away.
                        if e.click_count() >= 2 {
                            this.insert_timestamp(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::BOLD)
                                    .when(lit, |el| el.text_color(p.primary))
                                    .child(
                                        picked
                                            .map(|s| {
                                                timestamps::format_clock(s, style, now, crate::ui::text::twelve_hours())
                                            })
                                            .unwrap_or_else(|| t("chattools.timestamp.pickDateTime")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.2))
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(t(style_name(style))),
                            ),
                    )
                    // The check pops in by the one picked.
                    .when(lit, |el| {
                        el.child(motion::pop_in(
                            div().child(icon("check").size(px(16.0)).text_color(p.primary)),
                            SharedString::from(format!("time-style-check|{}", style.letter())),
                            (0.5, 0.5),
                            0.0,
                            0.0,
                        ))
                    }),
                SharedString::from(format!("time-style-in|{n}")),
                Duration::from_millis(60 + 25 * n as u64),
                0.0,
            ));
        }

        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_t_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.3))
            .px(px(12.0))
            .py(px(8.0))
            .child(
                div()
                    .id("time-token")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family("monospace")
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chattools.timestamp.sent")).build(window, cx))
                    .child(picked.map(|s| timestamps::token(s, chosen)).unwrap_or_else(|| " ".into())),
            )
            .child(
                primary_button("time-insert", t("chattools.timestamp.insertButton"), p)
                    .h(px(32.0))
                    .px(px(14.0))
                    .rounded(radius_xl())
                    .text_xs()
                    .when(picked.is_none(), |el| el.opacity(0.5))
                    .active(|s| s.scale(0.92))
                    .on_click(cx.listener(|this, _, window, cx| this.insert_timestamp(window, cx))),
            );

        let body = div()
            .w(px(336.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(vec![gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
                offset: gpui_kit::point(px(0.0), px(25.0)),
                blur_radius: px(50.0),
                spread_radius: px(-12.0),
                inset: false,
            }])
            .child(top)
            .child(styles)
            .child(footer);
        // Its right edge on the button's: the emoji button and the rest come after it.
        let right = self.tool_right(crate::ui::composer::Tool::Timestamp);
        Some(
            div()
                .id("time-panel")
                .absolute()
                .right(px(right))
                .bottom(gpui_kit::relative(1.0))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.close_time_picker(window, cx);
                }))
                // It grows from 92% as it rises from the button.
                .child(motion::pop_in(body.mb(px(-1.0)), "time-panel-rise", (1.0, 1.0), 0.92, 8.0))
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
