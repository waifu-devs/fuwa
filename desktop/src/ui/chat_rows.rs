//! The parts of the message list that aren't a message's own text, as the
//! web's `chat/MessageList.tsx` draws them: day dividers, the channel's
//! beginning, the loading shimmer, join lines with their wave, AutoMod's
//! alerts, the tools over a hovered message (and the "Delete?" they ask),
//! and the pill that jumps back to the newest messages.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Context, Div, FontWeight, HighlightStyle, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledText, WeakEntity, Window, div, px, rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::{AlertLine, JoinLine, RowCtx};
use crate::ui::context_menu::{self, MenuOf};
use crate::ui::motion;
use crate::ui::text::when;
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::{icon, name_tint};

/// Tailwind's amber-500, emerald-500 and friends, which the web uses for AutoMod and joins.
const AMBER_400: u32 = 0xfbbf24;
const AMBER_500: u32 = 0xf59e0b;
const AMBER_600: u32 = 0xd97706;
const ROSE_500: u32 = 0xf43f5e;
const EMERALD_500: u32 = 0x10b981;
const EMERALD_600: u32 = 0x059669;

/// Tailwind's `shadow-md`.
pub(crate) fn shadow_md() -> Vec<gpui_kit::BoxShadow> {
    let black = |a: f32| Hsla { h: 0.0, s: 0.0, l: 0.0, a };
    vec![
        gpui_kit::BoxShadow {
            color: black(0.1),
            offset: gpui_kit::point(px(0.0), px(4.0)),
            blur_radius: px(6.0),
            spread_radius: px(-1.0),
            inset: false,
        },
        gpui_kit::BoxShadow {
            color: black(0.1),
            offset: gpui_kit::point(px(0.0), px(2.0)),
            blur_radius: px(4.0),
            spread_radius: px(-2.0),
            inset: false,
        },
    ]
}

/// Tailwind's `shadow-lg`.
pub(crate) fn shadow_lg() -> Vec<gpui_kit::BoxShadow> {
    let black = |a: f32| Hsla { h: 0.0, s: 0.0, l: 0.0, a };
    vec![
        gpui_kit::BoxShadow {
            color: black(0.1),
            offset: gpui_kit::point(px(0.0), px(10.0)),
            blur_radius: px(15.0),
            spread_radius: px(-3.0),
            inset: false,
        },
        gpui_kit::BoxShadow {
            color: black(0.1),
            offset: gpui_kit::point(px(0.0), px(4.0)),
            blur_radius: px(6.0),
            spread_radius: px(-4.0),
            inset: false,
        },
    ]
}

/// A line between days (`DayDivider`): the day in small bold words between two rules.
pub(crate) fn day_row(text: &str, p: &Palette) -> AnyElement {
    div()
        .my(px(12.0))
        .px(px(16.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .text_xs()
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(div().flex_1().h(px(1.0)).bg(p.border))
        .child(text.to_owned())
        .child(div().flex_1().h(px(1.0)).bg(p.border))
        .into_any_element()
}

/// The top of a channel (`Beginning`): a sparkle in a soft circle, its welcome and how it starts.
pub(crate) fn beginning(
    id: &str,
    glyph: &str,
    title: &str,
    body: &str,
    extra: Option<AnyElement>,
    p: &Palette,
) -> AnyElement {
    motion::rise(
        div()
            .px(px(16.0))
            .pt(px(40.0))
            .pb(px(16.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(16.0))
                    .size(px(64.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .child(icon(glyph).size(px(32.0))),
            )
            .child(
                div()
                    .text_size(px(30.0))
                    .line_height(px(36.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(title.to_owned()),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .text_size(px(16.0))
                    .line_height(px(24.0))
                    .text_color(p.muted_foreground)
                    .child(body.to_owned()),
            )
            .children(extra),
        SharedString::from(format!("beginning|{id}")),
        Duration::ZERO,
        12.0,
    )
    .into_any_element()
}

/// The loading shimmer (`Skeleton`): faces and lines that shine while messages come.
pub(crate) fn skeleton(rows: usize, p: &Palette) -> AnyElement {
    let (base, shine) = (p.muted, mix(p.muted, p.card, 0.5));
    let block = move |id: String, el: Div| {
        el.bg(base).with_animation(
            SharedString::from(id),
            Animation::new(Duration::from_millis(1400)).repeat(),
            move |el, t| {
                let k = 1.0 - ((t * std::f32::consts::TAU).cos() * 0.5 + 0.5);
                el.bg(mix(base, shine.into(), k))
            },
        )
    };
    let mut list = div().flex().flex_col().gap(px(20.0)).px(px(16.0)).py(px(16.0));
    for n in 0..rows {
        let width = 40 + (n * 37) % 50;
        list = list.child(
            div()
                .flex()
                .gap(px(12.0))
                .opacity(1.0 - n as f32 * 0.12)
                .child(block(format!("sk-face-{n}"), div().size(px(40.0)).flex_none().rounded_full()))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .pt(px(4.0))
                        .child(block(format!("sk-name-{n}"), div().h(px(14.0)).w(px(128.0)).rounded(px(4.0))))
                        .child(block(
                            format!("sk-line-{n}"),
                            div().h(px(14.0)).w(gpui_kit::relative(width as f32 / 100.0)).rounded(px(4.0)),
                        )),
                ),
        );
    }
    list.into_any_element()
}

/// One of the tools over a hovered message (`ToolButton`): 32px, a 16px icon,
/// the muted fill on hover, or the destructive's for the dangerous ones.
pub(crate) fn tool(
    id: impl Into<SharedString>,
    glyph: &str,
    label: String,
    danger: bool,
    p: &Palette,
) -> Stateful<Div> {
    let (bg, fg) = if danger { (alpha(p.destructive, 0.15), p.destructive) } else { (p.muted.into(), p.foreground) };
    div()
        .id(id.into())
        .size(px(32.0))
        .flex_none()
        .rounded(radius_lg())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(p.muted_foreground)
        .hover(move |s| s.bg(bg).text_color(fg))
        .active(|s| s.opacity(0.8))
        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
        .child(icon(glyph).size(px(16.0)))
}

/// The card the tools sit in, over the message's top right corner. It keeps
/// the row lit while the pointer is on it, as the web's does.
pub(crate) fn tools_frame(id: &str, ctx: &Rc<RowCtx>, p: &Palette) -> Stateful<Div> {
    let (this, row) = (ctx.this.clone(), id.to_owned());
    div()
        .id(SharedString::from(format!("tools|{id}")))
        .absolute()
        .top(px(-12.0))
        .right(px(16.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .shadow(shadow_md())
        .on_hover(move |on, _, cx| {
            let _ = this.update(cx, |this, cx| this.hover_row(&row, *on, true, cx));
        })
}

/// Whether a row's tools show: it's hovered, or its card is, or it's asking something.
pub(crate) fn tools_shown(id: &str, ctx: &RowCtx) -> bool {
    ctx.hover.as_deref() == Some(id) || ctx.deleting.as_deref() == Some(id)
}

/// The "Delete?" a row asks before it goes: the question, yes and no.
pub(crate) fn confirm_delete(id: &str, keep_label: String, ctx: &Rc<RowCtx>, p: &Palette) -> AnyElement {
    let (yes, no) = (ctx.this.clone(), ctx.this.clone());
    let (a, b) = (id.to_owned(), id.to_owned());
    motion::slide_in(
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(
                div()
                    .px(px(8.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.destructive)
                    .child(t("chat.messages.deleteAsk")),
            )
            .child(tool(format!("del-yes|{id}"), "check", t("chat.messages.delete"), true, p).on_click(
                move |_, _, cx| {
                    let _ = yes.update(cx, |this, cx| {
                        this.msg_ui.deleting = None;
                        this.delete(a.clone(), cx);
                        cx.notify();
                    });
                },
            ))
            .child(tool(format!("del-no|{id}"), "x", keep_label, false, p).on_click(move |_, _, cx| {
                let _ = no.update(cx, |this, cx| {
                    if this.msg_ui.deleting.as_deref() == Some(b.as_str()) {
                        this.msg_ui.deleting = None;
                    }
                    cx.notify();
                });
            })),
        SharedString::from(format!("del-ask|{id}")),
        8.0,
    )
    .into_any_element()
}

/// A trash can that asks first, for rows that aren't ordinary messages (`DeleteTools`).
fn delete_tools(id: &str, ctx: &Rc<RowCtx>, p: &Palette) -> AnyElement {
    let frame = tools_frame(id, ctx, p);
    if ctx.deleting.as_deref() == Some(id) {
        return frame.child(confirm_delete(id, t("chat.messages.keep"), ctx, p)).into_any_element();
    }
    let (this, row) = (ctx.this.clone(), id.to_owned());
    frame
        .child(tool(format!("del|{id}"), "trash", t("chat.messages.delete"), true, p).on_click(move |_, _, cx| {
            let _ = this.update(cx, |this, cx| {
                this.msg_ui.deleting = Some(row.clone());
                cx.notify();
            });
        }))
        .into_any_element()
}

/// The web's `hueOf`: a stable number from an id.
fn hue_of(id: &str) -> u32 {
    id.chars().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32)) % 360
}

/// A translated line with its placeholders filled, some of them styled: the
/// text, its styled runs, and where each placeholder landed.
type Runs = Vec<(std::ops::Range<usize>, HighlightStyle)>;
fn fill(
    template: &str,
    values: &[(&str, &str, Option<HighlightStyle>)],
) -> (String, Runs, Vec<(String, std::ops::Range<usize>)>) {
    let (mut out, mut styles, mut places) = (String::new(), Vec::new(), Vec::new());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|c| open + c) else { break };
        let key = &rest[open + 1..close];
        out.push_str(&rest[..open]);
        match values.iter().find(|(k, ..)| *k == key) {
            Some((_, value, style)) => {
                let range = out.len()..out.len() + value.len();
                out.push_str(value);
                if let Some(style) = style {
                    styles.push((range.clone(), *style));
                }
                places.push((key.to_owned(), range));
            }
            None => out.push_str(&rest[open..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    (out, styles, places)
}

fn bold(color: Option<Hsla>) -> HighlightStyle {
    HighlightStyle { color, font_weight: Some(FontWeight::BOLD), ..Default::default() }
}

/// A line naming someone, their name clickable to open their card.
fn naming_line(
    id: String,
    text: String,
    styles: Runs,
    name: Option<std::ops::Range<usize>>,
    user_id: &str,
    ctx: &Rc<RowCtx>,
) -> gpui_kit::InteractiveText {
    let (this, key, server, uid) = (ctx.this.clone(), ctx.key.clone(), ctx.server.clone(), user_id.to_owned());
    gpui_kit::InteractiveText::new(SharedString::from(id), StyledText::new(text).with_highlights(styles))
        .on_click(name.into_iter().collect(), move |_, window, cx| {
            open_person(&this, &key, server.clone(), uid.clone(), window, cx)
        })
}

/// Opens someone's card from a line that names them.
fn open_person(
    this: &WeakEntity<FuwaApp>,
    key: &str,
    server: Option<String>,
    user_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let key = key.to_owned();
    let _ = this.update(cx, |this, cx| {
        this.open_dialog(crate::ui::app::Dialog::Profile { key, user_id, server }, window, cx);
    });
}

/// "Someone joined", with a wave for them (`JoinRow`).
pub(crate) fn join_row(j: &JoinLine, ctx: &Rc<RowCtx>, p: &Palette) -> AnyElement {
    const LINES: [&str; 8] = [
        "chat.join.line1",
        "chat.join.line2",
        "chat.join.line3",
        "chat.join.line4",
        "chat.join.line5",
        "chat.join.line6",
        "chat.join.line7",
        "chat.join.line8",
    ];
    let color = j.color.unwrap_or_else(|| name_tint(&j.user_id, p));
    let template = t(LINES[(hue_of(&j.user_id) % LINES.len() as u32) as usize]);
    let (text, styles, places) = fill(&template, &[("name", &j.name, Some(bold(Some(color))))]);
    let name = places.into_iter().next().map(|(_, r)| r);
    let line = naming_line(format!("join-text|{}", j.id), text, styles, name, &j.user_id, ctx);
    let waved = ctx.waved.contains(&j.id);
    let waving = ctx.waving.contains(&j.id);
    let wave = (!j.mine && j.user.is_some() && j.can_wave).then(|| {
        let done = waved || waving;
        let (hover_border, hover_bg, hover_fg) = (alpha(p.primary, 0.4), alpha(p.primary, 0.1), p.primary);
        let hand = div().child("👋");
        let hand: AnyElement = if done {
            motion::once(
                hand,
                SharedString::from(format!("wave-hand|{}", j.id)),
                Duration::from_millis(800),
                |el, t| {
                    let k = [0.0, 22.0, -10.0, 22.0, -6.0, 0.0];
                    let x = t * 5.0;
                    let n = (x.floor() as usize).min(4);
                    let deg = k[n] + (k[n + 1] - k[n]) * (x - n as f32);
                    el.relative().left(px(deg / 22.0 * 1.5)).top(px(-deg.abs() / 22.0))
                },
            )
        } else {
            hand.into_any_element()
        };
        let mut button = div()
            .id(SharedString::from(format!("wave|{}", j.id)))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(4.0))
            .rounded_full()
            .border_1()
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .child(hand)
            .child(if waved { t("chat.join.waved") } else { t("chat.join.wave") });
        if waved {
            button = button
                .border_color(gpui_kit::transparent_black())
                .bg(alpha(rgb(EMERALD_500), 0.1))
                .text_color(rgb(EMERALD_600));
        } else {
            let (this, id, username) =
                (ctx.this.clone(), j.id.clone(), j.user.as_ref().map(|u| u.username.clone()).unwrap_or_default());
            button = button
                .border_color(p.border)
                .when(!waving, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.border_color(hover_border).bg(hover_bg).text_color(hover_fg))
                        .active(|s| s.opacity(0.85))
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| this.wave(id.clone(), username.clone(), cx));
                        })
                })
                .when(waving, |el| el.opacity(0.6));
        }
        button
    });
    let id = j.id.clone();
    let row = div()
        .id(SharedString::from(format!("join|{id}")))
        .group("join")
        .relative()
        .flex()
        .items_center()
        .gap(px(12.0))
        .px(px(16.0))
        .py(px(2.0))
        .min_h(px(30.0))
        .hover({
            let bg = alpha(p.muted, 0.45);
            move |s| s.bg(bg)
        })
        .on_hover({
            let (this, id) = (ctx.this.clone(), id.clone());
            move |on, _, cx| {
                let _ = this.update(cx, |this, cx| this.hover_row(&id, *on, false, cx));
            }
        })
        .on_mouse_down(
            MouseButton::Right,
            context_menu::on_right_click(ctx.this.clone(), {
                let (key, server, uid) = (ctx.key.clone(), ctx.server.clone(), j.user_id.clone());
                move |_, _| Some(MenuOf::Member { key: key.clone(), server: server.clone(), user_id: uid.clone() })
            }),
        )
        .child(
            div().w(px(40.0)).flex_none().flex().justify_center().child(
                div()
                    .relative()
                    .left(px(0.0))
                    .group_hover("join", |s| s.left(px(4.0)))
                    .child(icon("arrow-right").size(px(16.0)).text_color(rgb(EMERALD_500))),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_x(px(4.0))
                .text_size(px(15.04))
                .line_height(px(22.56))
                .text_color(p.muted_foreground)
                .child(line)
                .child(div().flex_none().text_xs().whitespace_nowrap().child(when(j.at))),
        )
        .children(wave);
    let row = if j.can_delete && tools_shown(&j.id, ctx) { row.child(delete_tools(&j.id, ctx, p)) } else { row };
    row.into_any_element()
}

/// The words of an alert in its quote, with what set the rule off marked.
fn marked(text: &str, matched: &[String], p: &Palette) -> StyledText {
    let mut words: Vec<String> = matched
        .iter()
        .filter(|m| {
            !m.is_empty()
                && !(m.ends_with(" pings") && m.trim_end_matches(" pings").chars().all(|c| c.is_ascii_digit()))
        })
        .map(|m| m.to_lowercase())
        .collect();
    words.sort_by_key(|w| std::cmp::Reverse(w.len()));
    let lower = text.to_lowercase();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    // Lowercasing can change lengths outside ASCII; mark only where the byte offsets still line up.
    if lower.len() == text.len() {
        let mut at = 0;
        while at < lower.len() {
            let hit = words.iter().find(|w| lower[at..].starts_with(w.as_str()));
            match hit {
                Some(w) => {
                    ranges.push(at..at + w.len());
                    at += w.len();
                }
                None => at += lower[at..].chars().next().map_or(1, char::len_utf8),
            }
        }
    }
    let style = HighlightStyle {
        color: Some(p.destructive.into()),
        background_color: Some(alpha(p.destructive, 0.2)),
        ..Default::default()
    };
    StyledText::new(text.to_owned()).with_highlights(ranges.into_iter().map(|r| (r, style)))
}

/// A small rounded chip under an alert: the rule, a matched word, a time out.
fn chip(glyph: Option<&str>, text: String, fg: Hsla, bg: Hsla, round: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(if round { 8.0 } else { 6.0 }))
        .py(px(2.0))
        .map(|el| if round { el.rounded_full() } else { el.rounded(px(6.0)) })
        .bg(bg)
        .text_color(fg)
        .when_some(glyph, |el, g| el.child(icon(g).size(px(12.0))))
        .child(text)
}

/// "N minutes", "1 hour", "7 days": a length of time in its largest whole unit.
fn duration(seconds: i64) -> String {
    let (n, unit) = [(86_400, "day"), (3_600, "hour"), (60, "minute")]
        .into_iter()
        .find(|(s, _)| seconds >= *s && seconds % s == 0)
        .map_or((seconds, "second"), |(s, u)| (seconds / s, u));
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// What AutoMod caught (`AutoModAlertRow`): who, where, what they said and what was done.
pub(crate) fn alert_row(a: &AlertLine, ctx: &Rc<RowCtx>, p: &Palette) -> AnyElement {
    let alert = &a.alert;
    let amber = rgb(AMBER_500);
    let amber_fg: Hsla = if p.dark { rgb(AMBER_400).into() } else { rgb(AMBER_600).into() };
    let badge = div().w(px(40.0)).flex_none().pt(px(4.0)).child(
        div()
            .size(px(40.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::linear_gradient(
                135.0,
                gpui_kit::linear_color_stop(rgb(AMBER_400), 0.0),
                gpui_kit::linear_color_stop(rgb(ROSE_500), 1.0),
            ))
            .text_color(rgb(0xffffff))
            .shadow(shadow_md())
            .child(icon("shield-alert").size(px(20.0))),
    );
    let head = div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(8.0))
        .text_sm()
        .line_height(px(20.0))
        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("chat.automod.name")))
        .child(
            div()
                .px(px(4.0))
                .rounded(px(4.0))
                .bg(alpha(p.primary, 0.15))
                .text_color(p.primary)
                .text_size(px(9.6))
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(t("chat.automod.bot").to_uppercase()),
        )
        .child(div().text_xs().text_color(p.muted_foreground).child(when(a.at)));
    let rule = chip(Some("shield"), alert.rule_name.clone(), p.foreground.into(), p.muted.into(), true)
        .font_weight(FontWeight::BOLD);
    let mut chips = div().mt(px(8.0)).flex().flex_wrap().items_center().gap(px(6.0)).text_xs().child(rule);
    let mut lines = div().text_sm().line_height(px(20.0));
    if alert.capped_per_day > 0 {
        let count = alert.capped_per_day.to_string();
        let template = t_with("chat.automod.capped", &[("count", Arg::Num(alert.capped_per_day))]);
        let at = template.find(&count);
        let text = StyledText::new(template.clone()).with_highlights(at.map(|at| {
            (at..at + count.len(), HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() })
        }));
        lines = lines.child(text);
        chips = chips.child(
            chip(Some("timer"), t("chat.automod.backAtMidnight"), amber_fg, alpha(amber, 0.15), true)
                .font_weight(FontWeight::BOLD),
        );
    } else {
        let key = match (alert.blocked, a.channel.is_some()) {
            (true, true) => "chat.automod.blockedIn",
            (true, false) => "chat.automod.blocked",
            (false, true) => "chat.automod.flaggedIn",
            (false, false) => "chat.automod.flagged",
        };
        let reason = t(match pb::AutoModTrigger::try_from(alert.trigger) {
            Ok(pb::AutoModTrigger::Keywords) => "chat.automod.reason.keywords",
            Ok(pb::AutoModTrigger::MentionSpam) => "chat.automod.reason.mentionSpam",
            Ok(pb::AutoModTrigger::Links) => "chat.automod.reason.links",
            _ => "chat.automod.reason.other",
        });
        let channel = a.channel.as_ref().map(|c| format!("#{c}")).unwrap_or_default();
        let color = a.color.unwrap_or_else(|| name_tint(&a.user_id, p));
        let (text, styles, places) = fill(
            &t(key),
            &[
                ("name", &a.name, Some(bold(Some(color)))),
                ("channel", &channel, Some(bold(None))),
                ("reason", &reason, None),
            ],
        );
        let name = places.into_iter().find(|(k, _)| k == "name").map(|(_, r)| r);
        lines = lines.child(naming_line(format!("alert-text|{}", a.id), text, styles, name, &a.user_id, ctx)).child(
            div()
                .id(SharedString::from(format!("alert-quote|{}", a.id)))
                .mt(px(8.0))
                .max_h(px(160.0))
                .overflow_y_scroll()
                .rounded(radius_xl())
                .bg(alpha(p.muted, 0.6))
                .px(px(12.0))
                .py(px(8.0))
                .text_color(p.muted_foreground)
                .child(marked(&alert.content, &alert.matched, p)),
        );
        for m in &alert.matched {
            chips = chips.child(
                chip(None, m.clone(), p.destructive.into(), alpha(p.destructive, 0.1), false).font_family("monospace"),
            );
        }
        if alert.timed_out_seconds > 0 {
            let span = duration(i64::from(alert.timed_out_seconds));
            chips = chips.child(
                chip(
                    Some("timer"),
                    t_with("chat.automod.timedOutFor", &[("duration", Arg::Str(&span))]),
                    amber_fg,
                    alpha(amber, 0.15),
                    true,
                )
                .font_weight(FontWeight::BOLD),
            );
        }
    }
    // The card: a 4px amber edge on the left, the border around the rest.
    let surface = mix(p.background, p.card, 0.7);
    let card = div().mt(px(4.0)).rounded(radius_2xl()).bg(amber).pl(px(4.0)).child(
        div()
            .rounded_r(radius_2xl())
            .rounded_l(px(f32::from(radius_2xl()) - 4.0))
            .border_1()
            .border_l_0()
            .border_color(p.border)
            .bg(surface)
            .p(px(12.0))
            .child(lines)
            .child(chips),
    );
    let id = a.id.clone();
    let row = div()
        .id(SharedString::from(format!("alert|{id}")))
        .relative()
        .flex()
        .gap(px(12.0))
        .px(px(16.0))
        .py(px(2.0))
        .hover({
            let bg = alpha(p.muted, 0.45);
            move |s| s.bg(bg)
        })
        .on_hover({
            let (this, id) = (ctx.this.clone(), id.clone());
            move |on, _, cx| {
                let _ = this.update(cx, |this, cx| this.hover_row(&id, *on, false, cx));
            }
        })
        .child(badge)
        .child(div().flex_1().min_w_0().child(head).child(card));
    let row = if a.can_delete && tools_shown(&a.id, ctx) { row.child(delete_tools(&a.id, ctx, p)) } else { row };
    row.into_any_element()
}

/// Back to the newest messages (`JumpButton`), counting the ones that came in meanwhile.
pub(crate) fn jump_pill(missed: usize, p: &Palette, cx: &mut Context<FuwaApp>) -> AnyElement {
    let label = if missed > 0 {
        t_with("chat.messages.newMessages", &[("count", Arg::Num(missed as i64))])
    } else {
        t("chat.messages.jumpToPresent")
    };
    let arrow = icon("arrow-down").size(px(16.0)).with_animation(
        "jump-bounce",
        Animation::new(Duration::from_millis(1000)).repeat(),
        |el, t| {
            // Tailwind's animate-bounce: up a quarter of its size and back, easing at each end.
            let k = (t * std::f32::consts::TAU).cos() * 0.5 + 0.5;
            el.relative().top(px(-4.0 * k))
        },
    );
    div()
        .absolute()
        .bottom(px(12.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(motion::rise(
            div()
                .id("jump-present")
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(16.0))
                .py(px(8.0))
                .rounded_full()
                .bg(p.primary)
                .text_color(p.primary_foreground)
                .text_sm()
                .line_height(px(20.0))
                .font_weight(FontWeight::BOLD)
                .shadow(shadow_lg())
                .cursor_pointer()
                .active(|s| s.opacity(0.9))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.msg_ui.missed = 0;
                    this.scroller.update(cx, |s, cx| s.scroll_to_end(cx));
                    cx.notify();
                }))
                .child(arrow)
                .child(label),
            "jump-present-in",
            Duration::ZERO,
            16.0,
        ))
        .into_any_element()
}

impl FuwaApp {
    /// A row (or its tools' card) under the pointer, or not: what shows its tools.
    pub(crate) fn hover_row(&mut self, id: &str, on: bool, tools: bool, cx: &mut Context<Self>) {
        let ui = &mut self.msg_ui;
        let before = ui.hovered.clone().or_else(|| ui.hovered_tools.clone());
        let slot = if tools { &mut ui.hovered_tools } else { &mut ui.hovered };
        if on {
            *slot = Some(id.to_owned());
        } else if slot.as_deref() == Some(id) {
            *slot = None;
        }
        let after = ui.hovered.clone().or_else(|| ui.hovered_tools.clone());
        if before != after {
            cx.notify();
        }
    }

    /// Waves at someone who joined: "👋 @them" in the channel.
    pub(crate) fn wave(&mut self, join_id: String, username: String, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        if !self.msg_ui.waving.insert(join_id.clone()) {
            return;
        }
        cx.notify();
        let core = self.core.clone();
        let text = format!("👋 @{username}");
        self.run(
            cx,
            async move { core.send_message(&key, &server, &channel, &text).await },
            move |this, result, cx| {
                this.msg_ui.waving.remove(&join_id);
                match result {
                    Ok(()) => {
                        this.msg_ui.waved.insert(join_id);
                    }
                    Err(err) => this.toast("circle-alert", err.message, String::new(), None, None, cx),
                }
                cx.notify();
            },
        );
    }

    /// Copies a message's text, its button showing a check for a moment.
    pub(crate) fn copy_message_text(&mut self, id: String, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        let at = Instant::now();
        self.msg_ui.copied = Some((id, at));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.msg_ui.copied.as_ref().is_some_and(|(_, when)| *when == at) {
                    this.msg_ui.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
