//! Time out, kick, ban or rename someone: the dialog their card, their menu
//! and the Members page open, like the web app's `ModerateDialog.tsx`. Each
//! asks for a reason, kept in the server's audit log.

use std::time::Duration;

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, BoxShadow, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, point, px, rgb,
};

use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::moderation::{Action, timed_out_until};
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::{avatar, icon, pal};

/// Discord's time-out lengths.
const TIME_OUT: [i64; 6] = [60, 5 * 60, 10 * 60, 60 * 60, 86_400, 7 * 86_400];

/// How much of a banned person's history goes with them.
const DELETE: [(i64, &str); 6] = [
    (0, "workspace.moderate.purge.keep"),
    (3_600, "workspace.moderate.purge.hour"),
    (6 * 3_600, "workspace.moderate.purge.hours6"),
    (86_400, "workspace.moderate.purge.hours24"),
    (3 * 86_400, "workspace.moderate.purge.days3"),
    (7 * 86_400, "workspace.moderate.purge.days7"),
];

/// A length of time in its largest whole unit, as the web's `formatDuration`:
/// "30 seconds", "5 minutes", "1 hour", "7 days".
pub fn duration(seconds: i64) -> String {
    let (n, unit) = match seconds {
        s if s > 0 && s % 86_400 == 0 => (s / 86_400, "day"),
        s if s > 0 && s % 3_600 == 0 => (s / 3_600, "hour"),
        s if s > 0 && s % 60 == 0 => (s / 60, "minute"),
        s => (s, "second"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Time left, as a countdown (the web's `formatLeft`): "0:42", "12:05", "3h 20m", "2d 4h".
pub fn left(ms: i64) -> String {
    let s = ((ms.max(0) + 999) / 1000).max(0);
    if s < 3_600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3_600, s % 3_600 / 60)
    } else {
        format!("{}d {}h", s / 86_400, s % 86_400 / 3_600)
    }
}

/// When it ends, in local time: "today at 16:30", else "Tue 4 Oct at 16:30".
pub(crate) fn stamp(ms: i64) -> String {
    use chrono::TimeZone as _;
    let Some(at) = chrono::Local.timestamp_millis_opt(ms).single() else { return String::new() };
    if at.date_naive() == chrono::Local::now().date_naive() {
        at.format("today at %H:%M").to_string()
    } else {
        at.format("%a %-d %b at %H:%M").to_string()
    }
}

/// The web's `shadow-2xl`.
fn shadow_2xl() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
        offset: point(px(0.0), px(25.0)),
        blur_radius: px(50.0),
        spread_radius: px(-12.0),
        inset: false,
    }]
}

/// A shadcn `Label` with `font-bold`.
fn label(text: impl Into<SharedString>) -> gpui_kit::Div {
    div().text_sm().line_height(px(14.0)).font_weight(FontWeight::BOLD).child(text.into())
}

/// The amber of time-outs (`text-amber-600`, `-400` in the dark).
pub(crate) fn amber(p: &Palette) -> Hsla {
    if p.dark { rgb(0xfbbf24).into() } else { rgb(0xd97706).into() }
}

impl FuwaApp {
    /// The dialog for `action` on someone.
    pub(crate) fn render_moderate(
        &mut self,
        key: &str,
        server: &str,
        user_id: &str,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        // A new dialog starts with an empty reason, in focus (the nickname box takes it for a rename).
        let tag = format!("{key}|{server}|{user_id}|{}", kind_of(action));
        if self.people.reason_for.as_deref() != Some(tag.as_str()) {
            self.people.reason_for = Some(tag);
            self.people.reason.update(cx, |s, cx| s.set_value("", window, cx));
            if action == Action::Nickname {
                let nick = member.as_ref().map(|m| m.nickname.clone()).unwrap_or_default();
                let plain = user.as_ref().map(crate::core::store::user_name).unwrap_or_default();
                self.dialog_input.update(cx, |s, cx| {
                    s.set_value(nick, window, cx);
                    s.set_placeholder(plain, window, cx);
                    s.focus(window, cx);
                });
            } else {
                self.people.reason.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        if until.is_some() {
            // The countdown ticks.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }

        let (title, about) = match action {
            Action::TimeOut(_) => ("workspace.moderate.title.timeout", "workspace.moderate.about.timeout"),
            Action::Kick => ("workspace.moderate.title.kick", "workspace.moderate.about.kick"),
            Action::Ban(_) => ("workspace.moderate.title.ban", "workspace.moderate.about.ban"),
            Action::Nickname => ("workspace.moderate.title.nickname", "workspace.moderate.about.nickname"),
        };
        let header = div()
            .mb(px(20.0))
            .pr(px(32.0))
            .child(
                div()
                    .text_xl()
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(t_with(title, &[("name", Arg::Str(&name))])),
            )
            .child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(t(about)));

        // Who it's about, with how long their time-out has left.
        let who = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(radius_2xl())
            .bg(alpha(p.muted, 0.6))
            .child(avatar(user.as_ref(), 40.0, &p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::BOLD).truncate().child(name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .truncate()
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", user.as_ref().map(|u| u.username.as_str()).unwrap_or(""))),
                    ),
            )
            .when_some(until, |el, until| {
                let amber = amber(&p);
                el.child(crate::ui::motion::spring_in(
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
                        // The web's `animate-[spin_3s_ease-in-out_infinite]`.
                        .child(motion::ambient(
                            div().child(icon("hourglass").size(px(14.0))),
                            "mod-hourglass",
                            Duration::from_millis(3000),
                            window,
                            |el, t| {
                                let eased = if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 };
                                el.rotate(gpui_kit::radians(eased * std::f32::consts::TAU))
                            },
                        ))
                        .child(left(until - now)),
                    "mod-left",
                    (520.0, 34.0),
                    Duration::ZERO,
                    |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(0.6 + 0.4 * t),
                ))
            });

        let mut form = div().flex().flex_col().gap(px(16.0)).child(header).child(who);
        match action {
            Action::TimeOut(seconds) => {
                let chips: Vec<(i64, String)> = TIME_OUT.iter().map(|s| (*s, duration(*s))).collect();
                let ends = crate::ui::text::when(now + seconds * 1000);
                form = form.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(label(t("workspace.moderate.howLong")))
                        .child(self.chips("timeout", &chips, seconds, &p, cx, Action::TimeOut))
                        .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(
                            crate::ui::text::hint_line(
                                &t_with(
                                    if until.is_some() {
                                        "workspace.moderate.endsReplacing"
                                    } else {
                                        "workspace.moderate.ends"
                                    },
                                    &[("time", Arg::Str("{time}"))],
                                ),
                                &[("time", ends.as_str())],
                                &p,
                            ),
                        )),
                );
            }
            Action::Ban(purge) => {
                let chips: Vec<(i64, String)> = DELETE.iter().map(|(s, l)| (*s, t(l))).collect();
                form = form.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(label(t("workspace.moderate.purgeLabel")))
                        .child(self.chips("purge", &chips, purge, &p, cx, Action::Ban)),
                );
            }
            Action::Kick | Action::Nickname => {}
        }
        if action == Action::Nickname {
            form = form.child(
                div().flex().flex_col().gap(px(8.0)).child(label(t("workspace.moderate.nickname"))).child(
                    div()
                        .h(px(44.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .flex()
                        .items_center()
                        .text_sm()
                        .child(div().flex_1().child(Input::new(&self.dialog_input).appearance(false))),
                ),
            );
        } else {
            form = form.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label(t("workspace.moderate.reason")))
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(t("workspace.moderate.reasonHint")),
                            ),
                    )
                    .child(
                        div()
                            .min_h(px(64.0))
                            .rounded(radius_xl())
                            .border_1()
                            .border_color(p.border)
                            .px(px(12.0))
                            .text_sm()
                            .child(
                                Textarea::new(&self.people.reason)
                                    .appearance(false)
                                    .text_size(px(14.0))
                                    .line_height(px(20.0)),
                            ),
                    ),
            );
        }
        if let Some(error) = self.dialog_error.clone() {
            form = form.child(div().text_sm().text_color(p.destructive).child(error));
        }

        // The buttons: end a running time-out on the left, then Cancel and the action.
        let ghost = |id: &'static str, text: String, glyph: Option<&'static str>| {
            let hover = p.accent;
            div()
                .id(id)
                .h(px(36.0))
                .px(px(if glyph.is_some() { 12.0 } else { 16.0 }))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_xl())
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(busy, |el| el.opacity(0.5))
                .hover(move |s| s.bg(hover))
                .active(|s| s.translate_y(px(1.0)))
                .when_some(glyph, |el, g| el.child(icon(g).size(px(16.0))))
                .child(text)
        };
        let (submit_glyph, submit_text) = match action {
            Action::TimeOut(_) if until.is_some() => ("hourglass", t("workspace.moderate.submit.changeTimeout")),
            Action::TimeOut(_) => ("hourglass", t("workspace.moderate.submit.timeout")),
            Action::Kick => ("door-open", t("workspace.moderate.submit.kick")),
            Action::Ban(_) => ("gavel", t("workspace.moderate.submit.ban")),
            Action::Nickname => ("pencil", t("workspace.moderate.submit.save")),
        };
        let (fill, fg) =
            if action == Action::Nickname { (p.primary, p.primary_foreground) } else { (p.destructive, rgb(0xffffff)) };
        let fill_hover = alpha(fill, 0.9);
        let submit = div()
            .id("mod-submit")
            .group("mod-submit")
            .h(px(36.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_xl())
            .bg(fill)
            .text_color(fg)
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .when(busy, |el| el.opacity(0.5))
            .hover(move |s| s.bg(fill_hover))
            .active(|s| s.translate_y(px(1.0)))
            .child(if busy {
                motion::ambient(
                    div().child(icon("loader-circle").size(px(16.0))),
                    "mod-busy",
                    Duration::from_millis(1000),
                    window,
                    |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
                )
            } else {
                // The icon acts out the action while pointed at: the hourglass turns
                // over, the door swings, the gavel lifts.
                div()
                    .id("mod-submit-icon")
                    .group_hover("mod-submit", move |s| match submit_glyph {
                        "hourglass" => s.rotate(gpui_kit::radians(std::f32::consts::PI)),
                        "door-open" => s.translate_x(px(2.0)),
                        "gavel" => s.rotate(gpui_kit::radians(-0.21)),
                        _ => s,
                    })
                    .child(icon(submit_glyph).size(px(16.0)))
                    .into_any_element()
            })
            .child(submit_text)
            .on_click(cx.listener(|this, _, window, cx| this.submit_moderation(window, cx)));
        let mut buttons = div().flex().flex_wrap().items_center().justify_end().gap(px(8.0));
        if matches!(action, Action::TimeOut(_)) && until.is_some() {
            let (k, s, u) = (key.to_owned(), server.to_owned(), user_id.to_owned());
            buttons = buttons.child(
                ghost("mod-end", t("workspace.moderate.endTimeout"), Some("timer-off")).mr_auto().on_click(
                    cx.listener(move |this, _, _, cx| {
                        if !this.dialog_busy {
                            this.moderate(k.clone(), s.clone(), u.clone(), Action::TimeOut(0), String::new(), cx)
                        }
                    }),
                ),
            );
        }
        buttons = buttons
            .child(
                ghost("mod-cancel", t("common.cancel"), None)
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog_now(cx))),
            )
            .child(submit);
        form = form.child(buttons);

        let fg = p.foreground;
        let muted = p.muted;
        let panel = div()
            .id("dialog-panel")
            .relative()
            .w(px(448.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(24.0))
            .shadow(shadow_2xl())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(form)
            .child(
                div()
                    .id("dialog-close")
                    .absolute()
                    .top(px(16.0))
                    .right(px(16.0))
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.bg(muted).text_color(fg))
                    .child(icon("x").size(px(16.0)))
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog_now(cx))),
            );
        motion::fade_in(
            div()
                .id("dialog-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
                .backdrop_blur(px(crate::ui::overlay::SCRIM_BLUR))
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| this.close_dialog_now(cx)))
                .child(crate::ui::overlay::roomy("dialog-room", motion::dialog_in(panel, "dialog-moderate"))),
            "dialog-fade-moderate",
            Duration::from_millis(200),
        )
        .into_any_element()
    }

    fn close_dialog_now(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        self.dialog_error = None;
        self.close_dialog(cx);
    }

    /// The dialog's button: does what it says, with the reason (or the new nickname).
    pub(crate) fn submit_moderation(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::Moderate { key, server, user_id, action }) = self.dialog.clone() else { return };
        if self.dialog_busy {
            return;
        }
        let text: String = if action == Action::Nickname {
            self.dialog_input.read(cx).value().trim().chars().take(32).collect()
        } else {
            self.people.reason.read(cx).value().trim().chars().take(512).collect()
        };
        self.moderate(key, server, user_id, action, text, cx);
    }

    /// A row of choices, the picked one filled with a check (the web's `Chips`).
    /// Picking one swaps the dialog's action for `make(value)`.
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
            .children(options.iter().map(|(value, text)| {
                let on = *value == picked;
                let value = *value;
                let (hover_border, fg) = (alpha(p.primary, 0.4), p.foreground);
                div()
                    .id(SharedString::from(format!("{id}-{value}")))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .px(px(12.0))
                    .py(px(4.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.border_color(p.primary).bg(p.primary).text_color(p.primary_foreground)
                        } else {
                            el.border_color(p.border)
                                .text_color(p.muted_foreground)
                                .hover(move |s| s.border_color(hover_border).text_color(fg))
                        }
                    })
                    .active(|s| s.scale(0.92))
                    .when(on, |el| {
                        el.child(crate::ui::motion::spring_in(
                            div().mr(px(4.0)).child(icon("check").size(px(12.0))),
                            SharedString::from(format!("{id}-on-{value}")),
                            (520.0, 34.0),
                            Duration::ZERO,
                            |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(t),
                        ))
                    })
                    .child(text.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(Dialog::Moderate { action, .. }) = &mut this.dialog {
                            *action = make(value);
                            cx.notify();
                        }
                    }))
            }))
            .into_any_element()
    }
}

/// What a toast says once it's done (the web's), none for a new nickname.
pub(crate) fn done_toast(action: Action, name: &str, deleted: i64) -> Option<(&'static str, String)> {
    let name = Arg::Str(name);
    Some(match action {
        Action::TimeOut(0) => ("message-circle", t_with("workspace.moderate.canTalk", &[("name", name)])),
        Action::TimeOut(s) => (
            "hourglass",
            t_with("workspace.moderate.timedOut", &[("name", name), ("duration", Arg::Str(&duration(s)))]),
        ),
        Action::Kick => ("door-open", t_with("workspace.moderate.kicked", &[("name", name)])),
        Action::Ban(_) if deleted > 0 => {
            ("gavel", t_with("workspace.moderate.bannedDeleted", &[("name", name), ("count", Arg::Num(deleted))]))
        }
        Action::Ban(_) => ("gavel", t_with("workspace.moderate.banned", &[("name", name)])),
        Action::Nickname => return None,
    })
}

/// Which dialog it is, apart from the length or purge picked in it.
fn kind_of(action: Action) -> &'static str {
    match action {
        Action::TimeOut(_) => "timeout",
        Action::Kick => "kick",
        Action::Ban(_) => "ban",
        Action::Nickname => "nickname",
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
        assert_eq!(duration(604_800), "7 days");
        assert_eq!(left(42_000), "0:42");
        assert_eq!(left(65_000), "1:05");
        assert_eq!(left(3 * 3_600_000 + 20 * 60_000), "3h 20m");
        assert_eq!(left(2 * 86_400_000 + 4 * 3_600_000), "2d 4h");
    }
}
