//! "fuwa 0.4.2 is out": shown at the top of General when the instance's daily
//! check (server/src/releases.rs) found a newer release, as the web's
//! `NewerRelease.tsx`. Nothing updates by itself; the links say where to read
//! about it and how, both on github.com.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::icon;

const GUIDE: &str = "https://github.com/waifu-devs/fuwa/blob/master/docs/self-hosting.md#updating";
const RELEASES: &str = "https://github.com/waifu-devs/fuwa/releases/";

/// A stamp that starts with "Today" or "Yesterday", lowercased to sit mid-sentence.
fn mid_sentence(stamp: String) -> String {
    for day in [t("common.time.today"), t("common.time.yesterday")] {
        if let Some(rest) = stamp.strip_prefix(day.as_str()) {
            return day.to_lowercase() + rest;
        }
    }
    stamp
}

/// A pill link that opens github.com.
fn link(id: &'static str, label: String, url: String, primary: bool, p: &Palette) -> impl IntoElement {
    div()
        .id(id)
        .h(px(32.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded_full()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .map(|el| {
            if primary {
                el.bg(p.primary).text_color(p.primary_foreground)
            } else {
                el.bg(p.secondary).text_color(p.foreground)
            }
        })
        .hover(|s| s.opacity(0.92))
        .active(|s| s.opacity(0.8))
        .on_click(move |_, _, cx| crate::ui::text::open_link(&url, cx))
        .child(label)
        .child(icon("arrow-up-right").size(px(14.0)))
}

/// The card, or nothing when this instance runs the latest release.
pub(super) fn newer_release(node: Option<&pb::Node>, p: &Palette) -> Option<AnyElement> {
    let node = node?;
    let release = node.versions.as_ref()?.newer_release.as_ref()?;
    if release.version.is_empty() {
        return None;
    }
    let when = release.published_at.as_ref().map(|at| {
        let ms = at.seconds * 1000 + i64::from(at.nanos) / 1_000_000;
        mid_sentence(crate::ui::text::when(ms))
    });
    let words = match &when {
        Some(when) => t_with(
            "instancesettings.release.runsCameOut",
            &[("current", Arg::Str(&node.version)), ("version", Arg::Str(&release.version)), ("when", Arg::Str(when))],
        ),
        None => t_with("instancesettings.release.runs", &[("current", Arg::Str(&node.version))]),
    };
    // Only GitHub's own release pages, whatever an instance says.
    let page = release.url.starts_with(RELEASES).then(|| release.url.clone());
    let gift = div()
        .size(px(40.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_xl())
        .bg(alpha(p.primary, 0.15))
        .text_color(p.primary)
        .child(icon("gift").size(px(20.0)));
    let card = div()
        .mb(px(20.0))
        .flex()
        .items_start()
        .gap(px(12.0))
        .p(px(16.0))
        .rounded(radius_2xl())
        .border_1()
        .border_color(alpha(p.primary, 0.3))
        .bg(alpha(p.primary, 0.08))
        .child(gift)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child(t_with("instancesettings.release.out", &[("version", Arg::Str(&release.version))])),
                )
                .child(div().mt(px(2.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(words))
                .child(
                    div()
                        .mt(px(12.0))
                        .flex()
                        .flex_wrap()
                        .gap(px(8.0))
                        .when_some(page, |el, page| {
                            el.child(link("release-page", t("instancesettings.release.whatsNew"), page, true, p))
                        })
                        .child(link("release-guide", t("instancesettings.release.howTo"), GUIDE.to_owned(), false, p)),
                ),
        );
    Some(motion::rise(card, SharedString::from("newer-release"), Duration::ZERO, 10.0).into_any_element())
}
