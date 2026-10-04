//! The Updates page: which version this is, whether a newer one is out, the
//! "Download updates in the background" switch, a look at what's new, and how updates reach
//! this computer (`core::updates`).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::config::Prefs;
use crate::core::updates::{self, Manual, Status};
use crate::ui::motion;
use crate::ui::settings::{SettingsView, section, toggle_row};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{error_line, icon, primary_button, soft_button};

impl SettingsView {
    pub(crate) fn updates_page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = updates::status();
        let courier = self
            .core
            .shared
            .read(|s| s.order.first().and_then(|key| s.instance(key)).map(|i| i.name()))
            .filter(|_| !prefs.streamer_mode)
            .unwrap_or_else(|| "your instance".into());

        let (glyph, tone, line) = match &status {
            Status::Idle | Status::Checking => ("refresh-cw", p.primary, "Looking for a new version…".to_owned()),
            Status::UpToDate => ("circle-check", p.success, "You're on the latest version.".to_owned()),
            Status::Available { release, .. } => ("gift", p.primary, format!("fuwa {} is out.", release.version)),
            Status::Downloading { release, .. } => {
                ("download", p.primary, format!("Downloading fuwa {}…", release.version))
            }
            Status::Ready { release } => {
                ("sparkles", p.primary, format!("fuwa {} is downloaded and checked.", release.version))
            }
            Status::Failed { .. } => ("circle-alert", p.destructive, "Couldn't update just now.".to_owned()),
        };
        let mark = div()
            .flex_none()
            .size(px(44.0))
            .rounded(corner(14.0))
            .bg(alpha(tone, 0.14))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(glyph).size(px(22.0)).text_color(tone));
        // Busy pulses; news pops in once.
        let mark = match &status {
            Status::Idle | Status::Checking | Status::Downloading { .. } => {
                motion::ambient(mark, "updates-busy", Duration::from_millis(1400), window, |el, t| {
                    el.opacity(0.55 + 0.45 * ((t * std::f32::consts::TAU).cos() * 0.5 + 0.5))
                })
            }
            _ => motion::once(
                mark,
                SharedString::from(format!("updates-mark-{glyph}")),
                Duration::from_millis(520),
                |el, t| {
                    let s = 1.0 - (1.0 - t).powi(3);
                    el.opacity(s).relative().top(px(6.0 * (1.0 - s)))
                },
            ),
        };

        let busy = matches!(status, Status::Checking | Status::Downloading { .. });
        let core = self.core.clone();
        let check = soft_button("updates-check", "Check now", p).when(!busy, |el| {
            el.on_click(move |_, _, _| {
                let core = core.clone();
                let install = core.prefs().auto_update;
                drop(Arc::clone(&core).spawn(async move { core.check_for_update(install).await }));
            })
        });
        let action: Option<AnyElement> = match &status {
            Status::Ready { .. } => Some(
                primary_button("updates-restart", "Restart to update", p)
                    .on_click(|_, window, cx| match updates::restart() {
                        Ok(()) => cx.quit(),
                        // Why it didn't is on the page now.
                        Err(_) => window.refresh(),
                    })
                    .into_any_element(),
            ),
            Status::Available { why: Manual::Off, .. } => {
                let core = self.core.clone();
                Some(
                    primary_button("updates-install", "Download it", p)
                        .on_click(move |_, _, _| {
                            let core = core.clone();
                            drop(Arc::clone(&core).spawn(async move { core.check_for_update(true).await }));
                        })
                        .into_any_element(),
                )
            }
            Status::Available { release, .. } => {
                let page = release.page.clone();
                // It names the site it opens: github.com sees whoever opens it.
                Some(
                    soft_button("updates-page", "Open the release on github.com", p)
                        .on_click(move |_, _, cx| cx.open_url(&page))
                        .into_any_element(),
                )
            }
            _ => None,
        };

        let mut head = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(18.0))
            .rounded(corner(18.0))
            .bg(p.card)
            .border_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(line))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(format!("This is fuwa desktop {}.", env!("CARGO_PKG_VERSION"))),
                            ),
                    )
                    .child(check),
            );
        if let Status::Downloading { done, total, .. } = &status {
            let fraction = if *total > 0 { (*done as f32 / *total as f32).clamp(0.0, 1.0) } else { 0.0 };
            let shown = motion::follow("updates-progress", fraction, window, cx);
            head =
                head.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div().h(px(8.0)).rounded_full().bg(p.secondary).overflow_hidden().child(
                                div().h_full().rounded_full().bg(p.primary).w(gpui_kit::relative(shown.max(0.02))),
                            ),
                        )
                        .child(div().text_xs().text_color(p.muted_foreground).child(match total {
                            0 => format!("{:.1} MB", *done as f64 / 1_048_576.0),
                            _ => format!("{:.1} of {:.1} MB", *done as f64 / 1_048_576.0, *total as f64 / 1_048_576.0),
                        })),
                );
        }
        if let Status::Ready { .. } = &status {
            head = head.child(div().text_sm().text_color(p.muted_foreground).child(
                "It only installs when you choose to. Until then this version keeps running, start after start.",
            ));
        }
        if let Status::Available { why, .. } = &status {
            head = head.child(div().text_sm().text_color(p.muted_foreground).child(why.explain()));
        }
        if let Status::Failed { what } = &status {
            head = head.when_some(error_line(Some(what), p), |el, line| el.child(line));
        }
        if let Some(action) = action {
            head = head.child(div().flex().child(action));
        }

        let how = div()
            .flex()
            .items_start()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(corner(14.0))
            .bg(p.secondary)
            .text_sm()
            .child(icon("shield-check").size(px(16.0)).mt(px(2.0)).text_color(p.primary))
            .child(div().flex_1().min_w_0().child(format!(
                "fuwa asks {courier} about new versions and downloads them through it, so GitHub, where releases live, never sees your address, and nothing about you goes with the request. Before an update runs, fuwa checks it against Waifu Devs' release signature and its SHA-256; one that doesn't match is thrown away. An update never installs or restarts the app by itself."
            )));

        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(28.0))
            .child(motion::rise(head, "updates-head", Duration::ZERO, 8.0))
            .child(toggle_row(
                "auto-update",
                "Download updates in the background",
                "New versions are fetched and checked so they're ready when you are; nothing installs until you press Restart to update. Off, fuwa only tells you when one is out.",
                prefs.auto_update,
                p,
                cx,
                |this, on, cx| this.set(cx, |pr| pr.auto_update = on),
            ));
        if let Some(release) = status.release().filter(|r| !r.notes.trim().is_empty()) {
            let notes = div()
                .id("updates-notes")
                .max_h(px(360.0))
                .overflow_y_scroll()
                .p(px(16.0))
                .rounded(corner(16.0))
                .bg(p.card)
                .border_1()
                .border_color(p.border)
                .text_sm()
                .child(crate::ui::text::markdown(
                    SharedString::from(format!("updates-notes-{}", release.version)),
                    crate::ui::text::images_as_links(&release.notes),
                ));
            page = page.child(section(&format!("What's new in {}", release.version), notes, p));
        }
        page.child(section("How updates work", how, p)).into_any_element()
    }
}
