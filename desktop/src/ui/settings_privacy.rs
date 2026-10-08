//! "Help fix bugs" on the Advanced page, as the web's `ShareReports`: the
//! switch for the anonymous reports (`core::reports`), what's sent and what
//! never is, and a live look at what's waiting to go out and where to.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, px,
};

use crate::core::config::Prefs;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::reports::{self, Pending};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::toggle;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_lg, radius_xl};
use crate::ui::widgets::icon;

impl SettingsView {
    pub(crate) fn reports_section(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = prefs.share_reports;
        let pending = if on { reports::pending() } else { Pending::default() };
        self.pending = pending;
        let target = self.core.report_destination().map(|key| {
            let name = self.core.shared.read(|s| s.instance(&key).map(|i| i.name()));
            name.filter(|_| !prefs.hides_personal()).unwrap_or_else(|| t("desktop.settings.yourInstance"))
        });
        let list = |title: String, glyph: &'static str, tone: gpui_kit::Rgba, lines: Vec<String>, start: usize| {
            let mut ul = div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .rounded(radius_xl())
                .bg(alpha(p.muted, 0.5))
                .px(px(12.0))
                .py(px(10.0))
                .text_sm()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(tracked(title.to_uppercase(), WIDE)),
                );
            for (n, line) in lines.into_iter().enumerate() {
                ul = ul.child(motion::slide_in(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(icon(glyph).size(px(14.0)).mt(px(3.0)).text_color(tone))
                        .child(div().flex_1().child(line)),
                    SharedString::from(format!("report-line-{}", start + n)),
                    -6.0,
                ));
            }
            ul
        };
        let sent = vec![
            t("desktop.privacy.sentErrors"),
            t("desktop.privacy.sentTimings"),
            t("desktop.privacy.sentUsage"),
            t("desktop.privacy.sentVersion"),
        ];
        let never = vec![
            t("appsettings.advanced.neverMessages"),
            t("appsettings.advanced.neverNames"),
            t("appsettings.advanced.neverAddress"),
        ];
        let lit = crate::ui::motion::follow("reports-lit", if on { 1.0 } else { 0.55 }, window, cx);
        let lists = div()
            .flex()
            .gap(px(8.0))
            .opacity(lit)
            .child(list(t("appsettings.advanced.sent"), "check", p.primary, sent, 0))
            .child(list(t("appsettings.advanced.never"), "x", p.destructive, never, 4));
        let chip = |glyph: &'static str, label: String, count: u32| {
            let active = count > 0;
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(radius_lg())
                .px(px(8.0))
                .py(px(4.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .map(|el| {
                    if active {
                        el.bg(alpha(p.primary, 0.12)).text_color(p.primary)
                    } else {
                        el.bg(p.muted).text_color(p.muted_foreground)
                    }
                })
                .child(icon(glyph).size(px(14.0)))
                .child(label)
        };
        let waiting = on.then(|| {
            motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(alpha(p.border, 0.6))
                    .bg(alpha(p.background, 0.6))
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_sm()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .child(tracked(t("appsettings.advanced.waiting").to_uppercase(), WIDE)),
                            )
                            .child(chip(
                                "bug",
                                t_with(
                                    "appsettings.advanced.waitingErrors",
                                    &[("count", Arg::Num(i64::from(pending.errors)))],
                                ),
                                pending.errors,
                            ))
                            .child(chip(
                                "gauge",
                                t_with(
                                    "appsettings.advanced.waitingTimings",
                                    &[("count", Arg::Num(i64::from(pending.timings)))],
                                ),
                                pending.timings,
                            ))
                            .child(chip(
                                "mouse-pointer-click",
                                t_with(
                                    "appsettings.advanced.waitingUsage",
                                    &[("count", Arg::Num(i64::from(pending.usage)))],
                                ),
                                pending.usage,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.0))
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(icon("server").size(px(14.0)).mt(px(1.0)))
                            .child(div().flex_1().child(match &target {
                                Some(name) => t_with("desktop.privacy.goesTo", &[("instance", Arg::Str(name))]),
                                None => t("desktop.privacy.nowhere"),
                            })),
                    ),
                "reports-waiting",
                Duration::ZERO,
                -6.0,
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(toggle(
                "share-reports",
                &t("appsettings.advanced.reportsToggle"),
                Some(&t("appsettings.advanced.reportsToggleHint")),
                on,
                false,
                p,
                window,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.share_reports = on),
            ))
            .child(lists)
            .children(waiting)
            .into_any_element()
    }
}
