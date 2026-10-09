//! Presence on screen, as the web app's `components/Presence.tsx`: the dot on
//! avatars, the one line under a name in the member list, and activity cards
//! on profile cards (`core::presence::people`).

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, img, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::text::ms_of;
use crate::ui::theme::{Palette, alpha, corner, mix, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, primary_button, soft_button};

/// What someone's dot shows: offline while they aren't kept as online.
pub fn shown(presence: Option<&pb::Presence>) -> pb::PresenceStatus {
    match presence.map(|p| p.status()) {
        Some(status @ (pb::PresenceStatus::Online | pb::PresenceStatus::Idle | pb::PresenceStatus::DoNotDisturb)) => {
            status
        }
        _ => pb::PresenceStatus::Offline,
    }
}

fn kind(activity: &pb::Activity) -> pb::ActivityKind {
    match activity.kind() {
        pb::ActivityKind::Unspecified => pb::ActivityKind::Playing,
        kind => kind,
    }
}

/// "Playing Celeste", "Listening to …", by kind, for under a name.
pub fn line(activity: &pb::Activity) -> String {
    let key = match kind(activity) {
        pb::ActivityKind::Streaming => "workspace.presence.line.streaming",
        pb::ActivityKind::Listening => "workspace.presence.line.listening",
        pb::ActivityKind::Watching => "workspace.presence.line.watching",
        pb::ActivityKind::Competing => "workspace.presence.line.competing",
        _ => "workspace.presence.line.playing",
    };
    t_with(key, &[("name", Arg::Str(&activity.name))])
}

/// An activity's line with its name in bold (`font-bold text-foreground/85`), as the web's `ActivityLine`.
pub fn rich_line(line: &str, name: &str, p: &Palette) -> gpui_kit::StyledText {
    let style = gpui_kit::HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        color: Some(alpha(p.foreground, 0.85)),
        ..Default::default()
    };
    let at = line.rfind(name).filter(|_| !name.is_empty());
    gpui_kit::StyledText::new(line.to_owned()).with_highlights(at.map(|at| (at..at + name.len(), style)))
}

fn heading(activity: &pb::Activity) -> String {
    t(match kind(activity) {
        pb::ActivityKind::Streaming => "workspace.presence.heading.streaming",
        pb::ActivityKind::Listening => "workspace.presence.heading.listening",
        pb::ActivityKind::Watching => "workspace.presence.heading.watching",
        pb::ActivityKind::Competing => "workspace.presence.heading.competing",
        _ => "workspace.presence.heading.playing",
    })
}

/// Pictures by key point at Discord, which apps never load: the kind stands in.
fn kind_icon(activity: &pb::Activity) -> &'static str {
    match kind(activity) {
        pb::ActivityKind::Streaming => "radio",
        pb::ActivityKind::Listening => "headphones",
        pb::ActivityKind::Watching => "tv",
        pb::ActivityKind::Competing => "trophy",
        _ => "gamepad-2",
    }
}

/// "4:05" or "1:02:03".
pub fn clock(ms: i64) -> String {
    let total = ms.max(0) / 1000;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// "12:04 elapsed" counting up, or "3:10 left" counting down; None without times.
pub fn timer(activity: &pb::Activity, now_ms: i64) -> Option<String> {
    let (start, end) = (ms_of(activity.started_at.as_ref()), ms_of(activity.ends_at.as_ref()));
    if end > 0 {
        Some(t_with("workspace.presence.left", &[("time", Arg::Str(&clock(end - now_ms)))]))
    } else if start > 0 {
        Some(t_with("workspace.presence.elapsed", &[("time", Arg::Str(&clock(now_ms - start)))]))
    } else {
        None
    }
}

/// Where a link someone's activity carries goes, when it's a web page this
/// app would open (https only): its host as the browser reads it (look-alike
/// letters spelled out as punycode), and the link as it will be opened, so
/// what's shown and what opens are the same.
pub fn https_target(url: &str) -> Option<(String, String)> {
    if !crate::core::commands::https_link(url) {
        return None;
    }
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str().filter(|h| !h.is_empty())?.to_owned();
    Some((host, parsed.as_str().to_owned()))
}

/// What someone's doing, on their profile card: one card per activity. A
/// button asks first, saying where it goes (`leaving` is the link asked about).
pub fn activity_cards(
    activities: &[pb::Activity],
    leaving: Option<&str>,
    now_ms: i64,
    p: &Palette,
    cx: &mut Context<FuwaApp>,
) -> Vec<AnyElement> {
    activities
        .iter()
        .enumerate()
        .map(|(n, a)| {
            let picture = |url: &str, size: f32, round: bool| {
                let el = img(SharedString::from(url.to_owned())).size(px(size));
                if round { el.rounded_full() } else { el.rounded(radius_xl()) }
            };
            // The web's `.activity-tile`: the theme's color sweeping toward a sky blue.
            let sky: gpui_kit::Hsla = mix(p.primary, gpui_kit::rgb(0x7dd3fc), 0.55);
            let tile = div()
                .relative()
                .flex_none()
                .child(if a.large_image_url.is_empty() {
                    div()
                        .size(px(64.0))
                        .rounded(radius_xl())
                        .bg(gpui_kit::linear_gradient(
                            135.0,
                            gpui_kit::linear_color_stop(p.primary, 0.0),
                            gpui_kit::linear_color_stop(sky, 1.0),
                        ))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.primary_foreground)
                        .child(icon(kind_icon(a)).size(px(32.0)))
                        .into_any_element()
                } else {
                    picture(&a.large_image_url, 64.0, false).into_any_element()
                })
                .when(!a.small_image_url.is_empty(), |el| {
                    el.child(
                        div()
                            .absolute()
                            .right(px(-9.0))
                            .bottom(px(-9.0))
                            .rounded_full()
                            .border_3()
                            .border_color(p.muted)
                            .child(picture(&a.small_image_url, 24.0, true)),
                    )
                });
            let party = (a.party_max > 0).then(|| {
                t_with(
                    "workspace.presence.party",
                    &[("size", Arg::Num(a.party_size.into())), ("max", Arg::Num(a.party_max.into()))],
                )
            });
            let state = match (a.state.is_empty(), party) {
                (true, None) => None,
                (_, None) => Some(a.state.clone()),
                (true, Some(party)) => Some(party),
                (false, Some(party)) => Some(format!("{} {party}", a.state)),
            };
            let text = div()
                .min_w_0()
                .flex()
                .flex_col()
                .text_sm()
                .line_height(px(19.25))
                .child(div().truncate().font_weight(FontWeight::EXTRA_BOLD).child(a.name.clone()))
                .when(!a.details.is_empty(), |el| el.child(div().truncate().child(a.details.clone())))
                .when_some(state, |el, s| el.child(div().truncate().child(s)))
                .when_some(timer(a, now_ms), |el, s| {
                    el.child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(s))
                });
            let mut card = div()
                .mt(px(12.0))
                .flex()
                .flex_col()
                .p(px(12.0))
                .rounded(radius_2xl())
                .bg(alpha(p.muted, 0.6))
                .child(crate::ui::profile_card::caps(&heading(a), p).mb(px(8.0)))
                .child(div().flex().items_center().gap(px(12.0)).child(tile).child(text));
            let mut buttons = div().mt(px(12.0)).flex().flex_col().gap(px(6.0));
            for (b, button) in a.buttons.iter().enumerate() {
                let id = format!("activity-{n}-{b}");
                if leaving == Some(button.url.as_str()) {
                    buttons = buttons.child(leave_box(&id, &button.url, p, cx));
                    continue;
                }
                let url = button.url.clone();
                let (rest, lit) = (alpha(p.background, 0.7), p.background);
                buttons = buttons.child(
                    div()
                        .id(SharedString::from(id))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .rounded(crate::ui::theme::radius_lg())
                        .bg(rest)
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_sm()
                        .line_height(px(20.0))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(move |s| s.bg(lit))
                        .active(|s| s.scale(0.98))
                        .child(div().truncate().child(button.label.clone()))
                        .child(div().flex_none().opacity(0.6).child(icon("external-link").size(px(14.0))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.profile_leaving = Some(url.clone());
                            cx.notify();
                        })),
                );
            }
            if !a.buttons.is_empty() {
                card = card.child(buttons);
            }
            card.into_any_element()
        })
        .collect()
}

/// Before following a link from someone's activity: where it goes, and that
/// the site will see this computer's address.
fn leave_box(id: &str, url: &str, p: &Palette, cx: &mut Context<FuwaApp>) -> AnyElement {
    let target = https_target(url);
    let title = t_with(
        "workspace.presence.leave.title",
        &[(
            "host",
            Arg::Str(
                &target.as_ref().map(|(h, _)| h.clone()).unwrap_or_else(|| t("workspace.presence.leave.thisLink")),
            ),
        )],
    );
    let shown = target.as_ref().map_or(url, |(_, href)| href.as_str()).to_owned();
    let open = target.map(|(h, href)| (t_with("workspace.presence.leave.open", &[("host", Arg::Str(&h))]), href));
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(10.0))
        .rounded(corner(12.0))
        .bg(alpha(p.background, 0.7))
        .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(title))
        .child(div().text_xs().text_color(p.muted_foreground).child(t("workspace.presence.leave.about")))
        .child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(corner(8.0))
                .bg(p.muted)
                .text_xs()
                .font_family("monospace")
                .child(shown),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.0))
                .child(
                    soft_button(SharedString::from(format!("{id}-stay")), t("workspace.presence.leave.stay"), p)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.profile_leaving = None;
                            cx.notify();
                        })),
                )
                .when_some(open, |el, (label, url)| {
                    el.child(primary_button(SharedString::from(format!("{id}-open")), label, p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.profile_leaving = None;
                            crate::core::reports::used("presence.button_open");
                            crate::ui::text::open_link(&url, cx);
                            cx.notify();
                        },
                    )))
                }),
        )
        .into_any_element()
}

/// Someone's dot on their avatar (the web's `PresenceDot`, `-right-0.5
/// -bottom-0.5`): a `size` dot in a `ring` of `under`, the color it sits on.
pub fn avatar_dot(
    status: pb::PresenceStatus,
    size: f32,
    ring: f32,
    under: gpui_kit::Hsla,
    p: &Palette,
) -> gpui_kit::Div {
    div().absolute().right(px(-2.0 - ring)).bottom(px(-2.0 - ring)).child(popping(status, size, ring, under, p))
}

/// A [`ringed_dot`] that pops in when it shows and whenever the status
/// changes (the web's `StatusDot`, keyed by its status).
pub fn popping(
    status: pb::PresenceStatus,
    size: f32,
    ring: f32,
    under: gpui_kit::Hsla,
    p: &Palette,
) -> impl IntoElement {
    crate::ui::profile_card::spring_in(
        ringed_dot(status, size, ring, under, p),
        SharedString::from(format!("presence-dot|{}", status as i32)),
        (600.0, 16.0),
        Duration::ZERO,
        |el, t| el.scale(0.3 + 0.7 * t).opacity((0.4 + 0.6 * t).clamp(0.0, 1.0)),
    )
}

/// The web's `.presence-dot` with `ring-[n]` around it: the ring is drawn
/// outside the dot, and an offline dot's middle stays clear, as on the web.
pub fn ringed_dot(
    status: pb::PresenceStatus,
    size: f32,
    ring: f32,
    under: gpui_kit::Hsla,
    p: &Palette,
) -> gpui_kit::Div {
    match status {
        // The web's mask cuts the ring away with the bite: only the crescent shows.
        pb::PresenceStatus::Idle => {
            return div()
                .flex_none()
                .size(px(size + 2.0 * ring))
                .p(px(ring))
                .child(gpui_kit::svg().path("fuwa/idle.svg").size(px(size)).text_color(gpui_kit::rgb(0xf5a524)));
        }
        // Offline's inset outline takes the ring's place (one box-shadow on the web): a hollow circle.
        pb::PresenceStatus::Offline | pb::PresenceStatus::Invisible => {
            return div()
                .flex_none()
                .size(px(size + 2.0 * ring))
                .p(px(ring))
                .child(crate::ui::user_menu::presence_dot(status, size, 0.0, under, p));
        }
        _ => {}
    }
    div()
        .flex_none()
        .size(px(size + 2.0 * ring))
        .rounded_full()
        .border(px(ring))
        .border_color(under)
        .child(crate::ui::user_menu::presence_dot(status, size, 0.0, under, p))
}

/// Someone's custom status while it lasts (the web's `shownStatus`).
pub fn custom_status(user: &pb::User, now: i64) -> Option<String> {
    if user.status.is_empty() {
        return None;
    }
    let ends = user.status_expires_at.as_ref().map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000);
    if ends.is_some_and(|e| e <= now) {
        return None;
    }
    Some(user.status.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_read_like_the_web() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(65_400), "1:05");
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(clock(-5_000), "0:00");
    }

    #[test]
    fn only_https_links_name_a_host() {
        let host = |url: &str| https_target(url).map(|(h, _)| h);
        assert_eq!(host("https://Example.com/play?x=1").as_deref(), Some("example.com"));
        assert_eq!(host("https://user@evil.example/").as_deref(), Some("evil.example"));
        assert_eq!(host("http://example.com"), None);
        assert_eq!(host("javascript:alert(1)"), None);
        assert_eq!(host("https:///nohost").as_deref(), Some("nohost"));
        assert_eq!(host("HTTPS://Example.com").as_deref(), Some("example.com"));
        assert_eq!(host("https://example.com/a b"), None);
        assert_eq!(host("https://example.com/\u{7}"), None);
        // A backslash ends the host, as in the browser: what's named is where it goes.
        let (h, href) = https_target("https://a.example\\@b.example/").unwrap();
        assert_eq!(h, "a.example");
        assert!(href.starts_with("https://a.example/"));
        // Look-alike letters show spelled out.
        assert_eq!(host("https://ехample.com/").as_deref(), Some("xn--ample-ywe6i.com"));
    }
}
