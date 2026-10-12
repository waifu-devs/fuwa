//! Reporting a message or a person to a server's moderators (docs/mod-view.md):
//! the dialog "Report message" and "Report" open, with a reason and a note,
//! then a thank-you once it's in. The person reported is never told.

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::mod_view::{MAX_REPORT_NOTE, REASONS, reason_hint_key, reason_key};
use crate::pb::ModReportReason as Reason;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::overlay::{dialog_button, dialog_card, dialog_close, dialog_header, dialog_layer};
use crate::ui::settings_controls::Look;
use crate::ui::theme::{Palette, alpha, radius_xl};
use crate::ui::widgets::{error_line, icon, pal};

/// What's being reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportOf {
    /// A message, with a little of what it says to show in the dialog.
    Message {
        channel: String,
        message: String,
        author: String,
        excerpt: String,
    },
    Member {
        user_id: String,
    },
}

impl ReportOf {
    fn who(&self) -> &str {
        match self {
            ReportOf::Message { author, .. } => author,
            ReportOf::Member { user_id } => user_id,
        }
    }

    fn tag(&self) -> String {
        match self {
            ReportOf::Message { message, .. } => format!("m|{message}"),
            ReportOf::Member { user_id } => format!("u|{user_id}"),
        }
    }
}

/// What the report dialog keeps while it's open.
pub struct Reporting {
    pub reason: Option<Reason>,
    pub note: Entity<TextareaState>,
    /// Which report the fields were last emptied for.
    pub for_tag: Option<String>,
    /// It went in: the dialog says thanks.
    pub sent: Option<Instant>,
}

impl Reporting {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> Self {
        let note = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(3, 6).placeholder(t("desktop.modView.report.notePlaceholder"))
        });
        Self { reason: None, note, for_tag: None, sent: None }
    }
}

/// A message's words, cut short for the dialog.
pub fn excerpt(content: &str) -> String {
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 160 { format!("{}…", flat.chars().take(159).collect::<String>()) } else { flat }
}

impl FuwaApp {
    /// Opens the report dialog for a message or a person.
    pub(crate) fn open_report(
        &mut self,
        key: &str,
        server: &str,
        of: ReportOf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_dialog(Dialog::Report { key: key.to_owned(), server: server.to_owned(), of }, window, cx);
    }

    pub(crate) fn render_report(
        &mut self,
        key: &str,
        server: &str,
        of: &ReportOf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        // A new report starts empty, its note in focus.
        let tag = format!("{key}|{server}|{}", of.tag());
        if self.report.for_tag.as_deref() != Some(tag.as_str()) {
            self.report.for_tag = Some(tag);
            self.report.reason = None;
            self.report.sent = None;
            self.report.note.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let name = self
            .core
            .shared
            .read(|s| s.instance(key).map(|i| i.display_name(Some(server), of.who())).unwrap_or_default());
        let close = cx.listener(|this, _, _, cx| {
            this.dialog = None;
            this.dialog_error = None;
            this.close_dialog(cx);
        });
        if let Some(at) = self.report.sent {
            return dialog_layer("report", self.report_sent(&name, at, &p, cx), &p, close, cx);
        }

        let (title, about) = match of {
            ReportOf::Message { .. } => (
                t("desktop.modView.report.messageTitle"),
                t_with("desktop.modView.report.messageAbout", &[("name", Arg::Str(&name))]),
            ),
            ReportOf::Member { .. } => (
                t_with("desktop.modView.report.memberTitle", &[("name", Arg::Str(&name))]),
                t("desktop.modView.report.memberAbout"),
            ),
        };
        let mut body = div().flex().flex_col().gap(px(14.0));
        if let ReportOf::Message { excerpt, .. } = of
            && !excerpt.is_empty()
        {
            body = body.child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .border_l_4()
                    .border_color(alpha(p.primary, 0.5))
                    .bg(alpha(p.muted, 0.6))
                    .text_sm()
                    .line_height(px(20.0))
                    .child(div().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(name.clone()))
                    .child(excerpt.clone()),
            );
        }
        body = body.child(label(t("desktop.modView.report.why")));
        let picked = self.report.reason;
        let mut reasons = div().flex().flex_col().gap(px(4.0));
        for (n, reason) in REASONS.into_iter().enumerate() {
            reasons = reasons.child(motion::slide_in_after(
                reason_row(reason, picked == Some(reason), &p, cx),
                SharedString::from(format!("report-reason-in-{n}")),
                -8.0,
                Duration::from_millis(20 * n as u64),
            ));
        }
        body = body.child(reasons);
        let typed = self.report.note.read(cx).value().chars().count();
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div().flex().items_center().justify_between().child(label(t("desktop.modView.report.note"))).child(
                        div()
                            .text_xs()
                            .text_color(if typed > MAX_REPORT_NOTE { p.destructive } else { p.muted_foreground })
                            .child(format!("{typed}/{MAX_REPORT_NOTE}")),
                    ),
                )
                .child(
                    div()
                        .min_h(px(72.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .px(px(12.0))
                        .text_sm()
                        .child(
                            Textarea::new(&self.report.note)
                                .appearance(false)
                                .text_size(px(14.0))
                                .line_height(px(20.0)),
                        ),
                ),
        );
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .text_color(p.muted_foreground)
                .child(icon("eye-off").size(px(14.0)))
                .child(t_with("desktop.modView.report.private", &[("name", Arg::Str(&name))])),
        );
        let busy = self.dialog_busy;
        let ready = picked.is_some() && !busy && typed <= MAX_REPORT_NOTE;
        let panel = dialog_card(false, &p)
            .child(dialog_header(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .size(px(36.0))
                            .flex_none()
                            .rounded(radius_xl())
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.destructive, 0.15))
                            .text_color(p.destructive)
                            .child(icon("flag").size(px(20.0))),
                    )
                    .child(div().flex_1().min_w_0().child(title)),
                Some(div().child(about).into_any_element()),
                &p,
            ))
            .child(body)
            .when_some(error_line(self.dialog_error.as_deref(), &p), |el, e| el.child(div().mt(px(12.0)).child(e)))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(dialog_button("report-cancel", t("common.cancel"), Look::Ghost, &p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.dialog = None;
                            this.close_dialog(cx);
                        },
                    )))
                    .child(
                        dialog_button(
                            "report-send",
                            if busy { t("desktop.modView.report.sending") } else { t("desktop.modView.report.send") },
                            Look::Destructive,
                            &p,
                        )
                        .when(!ready, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.send_report(cx))),
                    ),
            )
            .child(dialog_close("report-close", &p).on_click(cx.listener(|this, _, _, cx| {
                this.dialog = None;
                this.close_dialog(cx);
            })));
        dialog_layer("report", panel, &p, close, cx)
    }

    /// The thanks once a report is in: a check that pops, and a way out.
    fn report_sent(&self, name: &str, at: Instant, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let green: gpui_kit::Hsla = gpui_kit::rgb(0x10b981).into();
        dialog_card(false, p)
            .items_center()
            .child(motion::pop(
                div()
                    .size(px(56.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(green.opacity(0.15))
                    .text_color(green)
                    .child(icon("circle-check").size(px(28.0))),
                SharedString::from(format!("report-sent-{at:?}")),
                0.4,
                -20.0,
                Duration::ZERO,
            ))
            .child(motion::rise(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(t("desktop.modView.report.thanks")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(p.muted_foreground)
                            .child(t_with("desktop.modView.report.thanksAbout", &[("name", Arg::Str(name))])),
                    ),
                SharedString::from(format!("report-sent-words-{at:?}")),
                Duration::from_millis(80),
                8.0,
            ))
            .child(div().mt(px(20.0)).child(
                dialog_button("report-done", t("desktop.modView.report.done"), Look::Primary, p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.dialog = None;
                        this.close_dialog(cx);
                    },
                )),
            ))
            .into_any_element()
    }

    /// Sends the report: the reason picked and the note.
    pub(crate) fn send_report(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Report { key, server, of }) = self.dialog.clone() else { return };
        let Some(reason) = self.report.reason else { return };
        let note = self.report.note.read(cx).value().to_string();
        if self.dialog_busy || note.trim().chars().count() > MAX_REPORT_NOTE {
            return;
        }
        self.dialog_busy = true;
        self.dialog_error = None;
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                match &of {
                    ReportOf::Message { channel, message, .. } => {
                        core.report_message(&key, &server, channel, message, reason, &note).await
                    }
                    ReportOf::Member { user_id } => core.report_member(&key, &server, user_id, reason, &note).await,
                }
            },
            |this, result, cx| {
                this.dialog_busy = false;
                match result {
                    Ok(()) => this.report.sent = Some(Instant::now()),
                    Err(err) => this.dialog_error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }
}

fn label(text: String) -> gpui_kit::Div {
    div().text_sm().line_height(px(14.0)).font_weight(FontWeight::BOLD).child(text)
}

/// One reason to pick: its name and what it means, a check springing in on the picked one.
fn reason_row(reason: Reason, on: bool, p: &Palette, cx: &mut Context<FuwaApp>) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (hover, ring) = (alpha(p.muted, 0.7), alpha(p.primary, 0.5));
    div()
        .id(SharedString::from(format!("report-reason-{}", reason as i32)))
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(6.0))
        .rounded(radius_xl())
        .border_1()
        .cursor_pointer()
        .map(|el| {
            if on {
                el.border_color(ring).bg(alpha(p.primary, 0.1))
            } else {
                el.border_color(alpha(p.border, 0.0)).hover(move |s| s.bg(hover))
            }
        })
        .active(|s| s.scale(0.98))
        .child(
            div()
                .size(px(18.0))
                .flex_none()
                .rounded_full()
                .border_2()
                .border_color(if on { p.primary } else { p.border })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| {
                    el.bg(p.primary).child(motion::spring_in(
                        div().text_color(p.primary_foreground).child(icon("check").size(px(12.0))),
                        SharedString::from(format!("report-reason-on-{}", reason as i32)),
                        (520.0, 34.0),
                        Duration::ZERO,
                        |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(t),
                    ))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(t(reason_key(reason))))
                .child(div().text_xs().text_color(p.muted_foreground).child(t(reason_hint_key(reason)))),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.report.reason = Some(reason);
            cx.notify();
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts_are_one_short_line() {
        assert_eq!(excerpt("hello\n\n  there"), "hello there");
        let long = excerpt(&"a".repeat(400));
        assert_eq!(long.chars().count(), 160);
        assert!(long.ends_with('…'));
    }
}
