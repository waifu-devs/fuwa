//! Time out, kick or ban someone: the buttons on their card and the dialog
//! they open, like the web app's `ModerateDialog.tsx`. Each asks for a
//! reason, kept in the server's audit log.

use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::dms::now_ms;
use crate::core::moderation::{Action, timed_out_until};
use crate::pb::Permission as P;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, mix};
use crate::ui::widgets::{avatar, icon, labeled, pal, soft_button};

/// Discord's time-out lengths.
const TIME_OUT: [i64; 6] = [60, 5 * 60, 10 * 60, 60 * 60, 86_400, 7 * 86_400];

/// How much of a banned person's history goes with them.
const DELETE: [(i64, &str); 6] = [
    (0, "Keep it"),
    (3_600, "Last hour"),
    (6 * 3_600, "6 hours"),
    (86_400, "24 hours"),
    (3 * 86_400, "3 days"),
    (7 * 86_400, "7 days"),
];

/// "1 minute", "10 minutes", "1 hour", "1 day", "1 week".
pub fn duration(seconds: i64) -> String {
    let (n, unit) = match seconds {
        s if s % 604_800 == 0 => (s / 604_800, "week"),
        s if s % 86_400 == 0 => (s / 86_400, "day"),
        s if s % 3_600 == 0 => (s / 3_600, "hour"),
        s if s % 60 == 0 => (s / 60, "minute"),
        s => (s, "second"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// What's left of a time-out: "4d 2h", "3h 12m", "4m 05s".
pub fn left(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    let (d, h, m, s) = (s / 86_400, s / 3_600 % 24, s / 60 % 60, s % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m {s:02}s")
    }
}

/// When it ends, in local time: "16:30" today, else "Tue 4 Oct 16:30".
pub(crate) fn stamp(ms: i64) -> String {
    use chrono::TimeZone as _;
    let Some(at) = chrono::Local.timestamp_millis_opt(ms).single() else { return String::new() };
    if at.date_naive() == chrono::Local::now().date_naive() {
        at.format("today at %H:%M").to_string()
    } else {
        at.format("%a %-d %b at %H:%M").to_string()
    }
}

impl FuwaApp {
    /// The time out, kick and ban buttons on someone's card, for whichever
    /// you may do to them.
    pub(crate) fn moderation_buttons(
        &self,
        key: &str,
        server: &str,
        user_id: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let allowed = self.core.shared.read(|s| s.instance(key).map(|i| i.can_moderate(server, user_id)))?;
        if allowed.is_empty() {
            return None;
        }
        let mut row = div().flex().gap(px(8.0));
        for (n, permission) in allowed.into_iter().enumerate() {
            let (glyph, label, action, color) = match permission {
                P::TimeOutMembers => ("hourglass", "Time out", Action::TimeOut(3_600), p.primary),
                P::KickMembers => ("door-open", "Kick", Action::Kick, p.destructive),
                _ => ("gavel", "Ban", Action::Ban(0), p.destructive),
            };
            let (k, sid, uid) = (key.to_owned(), server.to_owned(), user_id.to_owned());
            let soft = alpha(color, 0.12);
            let strong = alpha(color, 0.2);
            row = row.child(motion::rise(
                div()
                    .id(SharedString::from(format!("mod-{label}")))
                    .flex_1()
                    .h(px(34.0))
                    .rounded(px(10.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .bg(alpha(color, 0.07))
                    .text_color(color)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(soft))
                    .active(move |s| s.bg(strong).top(px(1.0)))
                    .child(icon(glyph).size(px(14.0)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let dialog =
                            Dialog::Moderate { key: k.clone(), server: sid.clone(), user_id: uid.clone(), action };
                        this.open_dialog(dialog, window, cx)
                    })),
                SharedString::from(format!("mod-in-{label}")),
                Duration::from_millis(60 + 40 * n as u64),
                6.0,
            ));
        }
        Some(row.into_any_element())
    }

    /// The moderation dialog's icon, title, words, fields and button.
    pub(crate) fn moderate_parts(
        &mut self,
        key: &str,
        server: &str,
        user_id: &str,
        action: Action,
        cx: &mut Context<Self>,
    ) -> (&'static str, String, String, AnyElement, Option<&'static str>) {
        let p = pal(cx);
        let busy = self.dialog_busy;
        let (member, name) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, String::new()) };
            let member = i
                .members
                .get(server)
                .and_then(|l| l.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)))
                .cloned();
            (member, i.display_name(Some(server), user_id))
        });
        let now = now_ms();
        let until = member.as_ref().and_then(|m| timed_out_until(m, now));
        let user = member.as_ref().and_then(|m| m.user.clone());

        // Who it's about, with how long their time-out has left.
        let who = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(px(16.0))
            .bg(alpha(p.muted_foreground, 0.08))
            .child(avatar(user.as_ref(), 40.0, &p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::BOLD).text_ellipsis().child(name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", user.as_ref().map(|u| u.username.as_str()).unwrap_or(""))),
                    ),
            )
            .when_some(until, |el, until| {
                let amber = gpui_kit::hsla(0.11, 0.9, if p.dark { 0.62 } else { 0.42 }, 1.0);
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(amber.opacity(0.15))
                        .text_color(amber)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(icon("hourglass").size(px(13.0)).with_animation(
                            "mod-hourglass",
                            Animation::new(Duration::from_millis(1600)).repeat(),
                            |el, t| el.opacity(0.55 + 0.45 * (t * std::f32::consts::TAU).cos().abs()),
                        ))
                        .child(left(until - now)),
                )
            });

        let mut content = div().flex().flex_col().gap(px(16.0)).child(who);
        match action {
            Action::TimeOut(seconds) => {
                let chips = TIME_OUT.map(|s| (s, duration(s)));
                content = content.child(labeled(
                    "For how long",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(self.chips("timeout", &chips, seconds, &p, cx, Action::TimeOut))
                        .child(div().text_xs().text_color(p.muted_foreground).child(format!(
                            "Ends {}{}.",
                            stamp(now + seconds * 1000),
                            if until.is_some() { ", in place of the one running now" } else { "" }
                        ))),
                    &p,
                ));
            }
            Action::Ban(purge) => {
                let chips = DELETE.map(|(s, l)| (s, l.to_owned()));
                content = content.child(labeled(
                    "Delete their recent messages",
                    self.chips("purge", &chips, purge, &p, cx, Action::Ban),
                    &p,
                ));
            }
            Action::Kick => {}
        }
        content = content.child(labeled("Reason", Input::new(&self.dialog_input).large(), &p));
        if until.is_some() && matches!(action, Action::TimeOut(_)) {
            let (k, sid, uid) = (key.to_owned(), server.to_owned(), user_id.to_owned());
            content = content.child(
                soft_button("mod-end", "End their time-out now", &p).child(icon("timer-off").size(px(14.0))).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.moderate(k.clone(), sid.clone(), uid.clone(), Action::TimeOut(0), String::new(), cx)
                    }),
                ),
            );
        }
        match action {
            Action::TimeOut(_) => (
                "hourglass",
                format!("Time out {name}"),
                "They can still read, but can't send or edit messages until it ends.".into(),
                content.into_any_element(),
                Some(if busy { "Timing out…" } else { "Time out" }),
            ),
            Action::Kick => (
                "door-open",
                format!("Kick {name}"),
                "They leave the server. They can join again with an invite.".into(),
                content.into_any_element(),
                Some(if busy { "Kicking…" } else { "Kick" }),
            ),
            Action::Ban(_) => (
                "gavel",
                format!("Ban {name}"),
                "They leave the server and can't join again until someone unbans them.".into(),
                content.into_any_element(),
                Some(if busy { "Banning…" } else { "Ban" }),
            ),
        }
    }

    /// A row of choices, the picked one filled. Picking one swaps the
    /// dialog's action for `make(value)`.
    fn chips(
        &self,
        id: &'static str,
        options: &[(i64, String)],
        picked: i64,
        p: &Palette,
        cx: &mut Context<Self>,
        make: fn(i64) -> Action,
    ) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(options.iter().map(|(value, label)| {
                let on = *value == picked;
                let value = *value;
                let hover = mix(p.secondary, p.primary, 0.16);
                let chip = div()
                    .id(SharedString::from(format!("{id}-{value}")))
                    .h(px(30.0))
                    .px(px(12.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.bg(p.primary).text_color(p.primary_foreground)
                        } else {
                            el.bg(p.secondary).text_color(p.foreground).hover(move |s| s.bg(hover))
                        }
                    })
                    .active(|s| s.top(px(1.0)))
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(Dialog::Moderate { action, .. }) = &mut this.dialog {
                            *action = make(value);
                            cx.notify();
                        }
                    }));
                if on {
                    // The picked chip pops in each time it changes.
                    motion::rise(chip, SharedString::from(format!("{id}-on-{value}")), Duration::ZERO, 3.0)
                        .into_any_element()
                } else {
                    chip.into_any_element()
                }
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_like_words() {
        assert_eq!(duration(60), "1 minute");
        assert_eq!(duration(600), "10 minutes");
        assert_eq!(duration(3_600), "1 hour");
        assert_eq!(duration(86_400), "1 day");
        assert_eq!(duration(604_800), "1 week");
        assert_eq!(left(65_000), "1m 05s");
        assert_eq!(left(3 * 3_600_000 + 60_000), "3h 01m");
        assert_eq!(left(2 * 86_400_000 + 3_600_000), "2d 1h");
    }
}
