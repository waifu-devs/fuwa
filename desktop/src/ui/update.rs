//! The little card that says a new version of the app is ready (or out),
//! floating over the bottom of the sidebar so it covers nothing you're
//! using: "Restart to update", "What's new" (the Updates page in settings),
//! or, where the app can't put it in place itself, the way to get it.
//! Closing it hides it until the next start; a ready update still runs then.

use std::time::Duration;

use gpui_kit::{
    AnyElement, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use parking_lot::Mutex;

use crate::core::updates::{self, Manual, Status};
use crate::ui::motion;
use crate::ui::rail::RAIL;
use crate::ui::settings::Page;
use crate::ui::sidebar::SIDEBAR;
use crate::ui::theme::{alpha, corner};
use crate::ui::widgets::{card, icon, pal};

/// The version whose card was closed, until the app quits.
static CLOSED: Mutex<Option<String>> = Mutex::new(None);

/// The me panel's height at the bottom of the sidebar, which the card sits above.
const ME_PANEL: f32 = 60.0;

impl crate::ui::app::FuwaApp {
    pub(crate) fn render_update_note(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Option<AnyElement> {
        let status = updates::status();
        let (release, ready, why) = match &status {
            Status::Ready { release } => (release.clone(), true, None),
            Status::Available { release, why } => (release.clone(), false, Some(*why)),
            _ => return None,
        };
        if CLOSED.lock().as_deref() == Some(release.version.as_str()) {
            return None;
        }
        let p = pal(cx);
        let version = release.version.clone();

        let badge = div()
            .relative()
            .flex_none()
            .size(px(36.0))
            .rounded(corner(12.0))
            .bg(alpha(p.primary, 0.16))
            .flex()
            .items_center()
            .justify_center()
            // A ring breathes out from it now and then while the window's in front.
            .child(motion::ambient(
                div().absolute().inset_0().rounded(corner(12.0)).border_2().border_color(alpha(p.primary, 0.5)),
                "update-ring",
                Duration::from_millis(2600),
                window,
                |el, t| {
                    let k = (t / 0.45).clamp(0.0, 1.0);
                    el.opacity(0.9 * (1.0 - k)).top(px(-5.0 * k)).left(px(-5.0 * k)).size(px(36.0 + 10.0 * k))
                },
            ))
            .child(motion::once(
                div().child(icon(if ready { "sparkles" } else { "gift" }).size(px(18.0)).text_color(p.primary)),
                SharedString::from(format!("update-glyph-{version}")),
                Duration::from_millis(900),
                |el, t| {
                    // A little wiggle as it arrives.
                    let k = (t * 3.0 * std::f32::consts::PI).sin() * (1.0 - t);
                    el.relative().left(px(2.5 * k)).top(px(-2.0 * k.abs()))
                },
            ));

        let (title, line) = if ready {
            (format!("fuwa {version} is ready"), "Restart now, or it starts next time.")
        } else if why == Some(Manual::Off) {
            (format!("fuwa {version} is out"), "Install it when you like.")
        } else {
            (format!("fuwa {version} is out"), "See how to get it.")
        };

        let close = div()
            .id("update-close")
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .size(px(24.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(p.muted_foreground)
            .hover(|s| s.bg(alpha(gpui_kit::rgb(0x808080), 0.2)))
            .on_click({
                let version = version.clone();
                cx.listener(move |_, _, _, cx| {
                    *CLOSED.lock() = Some(version.clone());
                    cx.notify();
                })
            })
            .child(icon("x").size(px(14.0)));

        let small = |id: &'static str, label: &'static str, filled: bool| {
            let (bg, fg) = if filled { (p.primary, p.primary_foreground) } else { (p.secondary, p.foreground) };
            let hover = gpui_kit::Rgba { a: 0.88, ..bg };
            div()
                .id(id)
                .h(px(30.0))
                .px(px(12.0))
                .rounded(corner(9.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(bg)
                .text_color(fg)
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(|s| s.top(px(1.0)))
                .child(label)
        };
        let main = if ready {
            small("update-restart", "Restart", true)
                .on_click(|_, _, cx| {
                    if updates::restart().is_ok() {
                        cx.quit();
                    }
                })
                .into_any_element()
        } else if why == Some(Manual::Off) {
            let core = self.core.clone();
            small("update-install", "Install", true)
                .on_click(move |_, _, _| {
                    let core = core.clone();
                    drop(std::sync::Arc::clone(&core).spawn(async move { core.check_for_update(true).await }));
                })
                .into_any_element()
        } else {
            small("update-how", "How to get it", true)
                .on_click(cx.listener(|this, _, window, cx| this.open_updates_page(window, cx)))
                .into_any_element()
        };
        let whats_new = small("update-new", "What's new", false)
            .on_click(cx.listener(|this, _, window, cx| this.open_updates_page(window, cx)));

        let note = card(&p)
            .relative()
            .w(px(SIDEBAR - 16.0))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div().flex().items_center().gap(px(10.0)).pr(px(20.0)).child(badge).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(title))
                        .child(div().text_xs().text_color(p.muted_foreground).child(line)),
                ),
            )
            .child(div().flex().gap(px(6.0)).child(main).child(whats_new))
            .child(close);
        Some(
            div()
                .absolute()
                .left(px(RAIL + 8.0))
                .bottom(px(ME_PANEL + 8.0))
                .child(motion::rise(
                    note,
                    SharedString::from(format!("update-note-{version}-{ready}")),
                    Duration::from_millis(120),
                    18.0,
                ))
                .into_any_element(),
        )
    }

    /// Opens settings on the Updates page.
    fn open_updates_page(&mut self, window: &mut Window, cx: &mut gpui_kit::Context<Self>) {
        self.open_settings(window, cx);
        if let Some(settings) = &self.settings {
            settings.update(cx, |view, cx| {
                view.page = Page::Updates;
                cx.notify();
            });
        }
    }
}
