//! Server banners (docs/servers.md): a wide picture across the top of the
//! welcome and onboarding screens, kept on its focal point whatever shape
//! it's shown in and drifting slowly unless motion is reduced; or, for a
//! server without one, a gradient in its colors. A server's accent color
//! tints those screens. Like the web app's `join/Banner.tsx` and
//! `lib/banner.ts`.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Div, Hsla, ImgResourceLoader, IntoElement, ParentElement as _, Resource, SharedString,
    Styled as _, Window, div, hsla, img, linear_color_stop, linear_gradient, px,
};

use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, corner};
use crate::ui::widgets::server_icon;

/// The hue the web's `hueOf` gives an id, 0 to 359.
pub fn hue_of(id: &str) -> u32 {
    let mut h: u32 = 0;
    for c in id.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(c));
    }
    h % 360
}

/// The server's accent: the one it picked, or a color from its hue.
pub fn accent(server: &pb::Server) -> Hsla {
    match server.accent_color {
        Some(c) if c >= 0 => gpui_kit::rgb(c as u32 & 0xff_ffff).into(),
        _ => hsla(hue_of(&server.id) as f32 / 360.0, 0.70, 0.58, 1.0),
    }
}

/// Text and ticks drawn on the accent: dark on a light one, white otherwise.
pub fn on_accent(accent: Hsla) -> Hsla {
    if accent.l > 0.72 { hsla(0.0, 0.0, 0.12, 1.0) } else { gpui_kit::white() }
}

/// 0xRRGGBB as #rrggbb.
pub fn hex(color: i32) -> String {
    format!("#{:06x}", color as u32 & 0xff_ffff)
}

/// #rgb or #rrggbb (the # optional) as 0xRRGGBB.
pub fn parse_hex(text: &str) -> Option<i32> {
    let t = text.trim().trim_start_matches('#');
    if !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full = match t.len() {
        3 => t.chars().flat_map(|c| [c, c]).collect(),
        6 => t.to_owned(),
        _ => return None,
    };
    i32::from_str_radix(&full, 16).ok()
}

/// Where a picture of `iw`×`ih` goes to cover `w`×`h` with its focus (0 to
/// 100 each way) as near the middle as it can be: its left, top, width and height.
pub fn cover(iw: f32, ih: f32, w: f32, h: f32, focus: (i32, i32)) -> (f32, f32, f32, f32) {
    let scale = (w / iw.max(1.0)).max(h / ih.max(1.0));
    let (sw, sh) = (iw * scale, ih * scale);
    let fx = focus.0.clamp(0, 100) as f32 / 100.0;
    let fy = focus.1.clamp(0, 100) as f32 / 100.0;
    (-(sw - w) * fx, -(sh - h) * fy, sw, sh)
}

/// The banner, `w`×`h`, fading into `under` at the bottom when given.
pub fn server_banner(
    server: &pb::Server,
    w: f32,
    h: f32,
    under: Option<Hsla>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    server_banner_round(server, w, h, under, 0.0, window, cx)
}

/// [`server_banner`] with its top corners rounded by `top`, for the top of a
/// card (GPUI doesn't clip children to a rounded parent).
pub fn server_banner_round(
    server: &pb::Server,
    w: f32,
    h: f32,
    under: Option<Hsla>,
    top: f32,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let still = cx.reduce_motion();
    let mut el = div().relative().w(px(w)).h(px(h)).overflow_hidden().flex_none();
    let picture = (!server.banner_url.is_empty())
        .then(|| {
            let url = SharedString::from(server.banner_url.clone());
            let resource = Resource::Uri(url.clone().into());
            match window.use_asset::<ImgResourceLoader>(&resource, cx) {
                Some(Ok(image)) => {
                    let s = image.size(0);
                    Some((url, i32::from(s.width) as f32, i32::from(s.height) as f32))
                }
                _ => None,
            }
        })
        .flatten();
    el = match picture {
        Some((url, iw, ih)) => {
            let focus = (server.banner_focus_x, server.banner_focus_y);
            let id = SharedString::from(format!("banner-pan-{}-{w}-{h}", server.id));
            // A slow drift, there and back: a little larger than it must be, so the edges never show.
            let place = move |t: f32| {
                let grow = if still { 1.04 } else { 1.06 + 0.06 * t };
                let (left, top, sw, sh) = cover(iw, ih, w * grow, h * grow, focus);
                let drift = if still { 0.0 } else { (t - 0.5) * 2.0 * 0.012 };
                (left - (w * grow - w) / 2.0 + drift * w, top - (h * grow - h) / 2.0 - drift * h * 0.5, sw, sh)
            };
            let shown = div().absolute().inset_0().child({
                let (l, t, sw, sh) = place(0.0);
                img(url.clone()).absolute().left(px(l)).top(px(t)).w(px(sw)).h(px(sh))
            });
            let moving = if still {
                shown.into_any_element()
            } else {
                motion::ambient(div().absolute().inset_0(), id, Duration::from_secs(52), window, move |el, t| {
                    // There and back again over one period.
                    let t = 0.5 - 0.5 * (t * std::f32::consts::TAU).cos();
                    let (l, tp, sw, sh) = place(t);
                    el.child(img(url.clone()).absolute().left(px(l)).top(px(tp)).w(px(sw)).h(px(sh)))
                })
            };
            el.child(motion::fade_in(div().absolute().inset_0().child(moving), "banner-in", Duration::from_millis(600)))
        }
        None => el.child(gradient(server, w, h, top, still, window)),
    };
    // Keeps a close button readable over a bright picture, and fades into the card below.
    el.child(div().absolute().top_0().left_0().right_0().h(px(56.0)).rounded_t(px(top)).bg(linear_gradient(
        180.0,
        linear_color_stop(hsla(0.0, 0.0, 0.0, 0.35), 0.0),
        linear_color_stop(hsla(0.0, 0.0, 0.0, 0.0), 1.0),
    )))
    .when_some(under, |el, under| {
        el.child(div().absolute().bottom_0().left_0().right_0().h(px(h * 0.66)).bg(linear_gradient(
            0.0,
            linear_color_stop(under, 0.0),
            linear_color_stop(Hsla { a: 0.0, ..under }, 1.0),
        )))
    })
    .into_any_element()
}

/// A server without a banner: its accent and hue, two soft lights drifting
/// over them and a faint dot grid (the web's `Gradient` in `join/Banner.tsx`).
fn gradient(server: &pb::Server, w: f32, h: f32, top: f32, still: bool, window: &Window) -> AnyElement {
    let a = accent(server);
    let rgb: gpui_kit::Rgba = a.into();
    // color-mix(accent 85%, black), the accent at 45%, then the hue 50° on.
    let dark: Hsla = gpui_kit::Rgba { r: rgb.r * 0.85, g: rgb.g * 0.85, b: rgb.b * 0.85, a: 1.0 }.into();
    let other = hsla(((hue_of(&server.id) + 50) % 360) as f32 / 360.0, 0.70, 0.60, 1.0);
    // GPUI's gradients have two stops, so the second half is laid over the first.
    let base = div()
        .absolute()
        .inset_0()
        .child(div().absolute().inset_0().rounded_t(px(top)).bg(linear_gradient(
            120.0,
            linear_color_stop(dark, 0.0),
            linear_color_stop(a, 0.45),
        )))
        .child(div().absolute().inset_0().rounded_t(px(top)).bg(linear_gradient(
            120.0,
            linear_color_stop(Hsla { a: 0.0, ..a }, 0.45),
            linear_color_stop(other, 1.0),
        )));
    let lights = move |el: Div, t: f32| {
        let (lw, lh) = (w * 0.7 * (1.0 + 0.2 * t), h * 0.7 * (1.0 + 0.2 * t));
        let (dw, dh) = (w * 0.8 * (1.0 + 0.15 * t), h * 0.8 * (1.0 + 0.15 * t));
        el.child(crate::ui::instance_home::soft_glow(
            -w * 0.1 + t * w * 0.7 * 0.18 - (lw - w * 0.7) / 2.0,
            -h / 3.0 + t * h * 0.7 * 0.12 - (lh - h * 0.7) / 2.0,
            lw,
            lh,
            hsla(0.0, 0.0, 1.0, 0.25),
            64.0,
        ))
        .child(crate::ui::instance_home::soft_glow(
            w - w * 0.8 + w * 0.1 - t * w * 0.8 * 0.16 - (dw - w * 0.8) / 2.0,
            h - h * 0.8 + h * 0.5 - t * h * 0.8 * 0.1 - (dh - h * 0.8) / 2.0,
            dw,
            dh,
            hsla(0.0, 0.0, 0.0, 0.2),
            64.0,
        ))
    };
    let lit = if still {
        lights(div().absolute().inset_0(), 0.0)
    } else {
        div().absolute().inset_0().child(motion::ambient(
            div().absolute().inset_0(),
            SharedString::from(format!("banner-glow-{}", server.id)),
            Duration::from_secs(36),
            window,
            move |el, t| lights(el, 0.5 - 0.5 * (t * std::f32::consts::TAU).cos()),
        ))
    };
    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .child(base)
        .child(lit)
        .child(crate::ui::instance_home::dot_grid(0.25, gpui_kit::rgb(0x000000)))
        .into_any_element()
}

/// The top of the welcome and onboarding screens: the banner, the server's
/// icon popping in over its bottom edge, a line above the name ("Welcome
/// to"), the name and how many are in it.
pub fn banner_hero(
    server: &pb::Server,
    eyebrow: &str,
    w: f32,
    p: &Palette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let card: Hsla = p.card.into();
    let members = server.member_count;
    div()
        .flex()
        .flex_col()
        .child(server_banner(server, w, 148.0, Some(card), window, cx))
        .child(
            div()
                .relative()
                .mt(px(-44.0))
                .px(px(24.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(motion::once(
                    div()
                        .w(px(88.0))
                        .h(px(88.0))
                        .rounded(corner(26.0))
                        .border_4()
                        .border_color(p.card)
                        .bg(p.card)
                        .child(server_icon(server, 80.0, 22.0, p)),
                    SharedString::from(format!("banner-icon-{}", server.id)),
                    Duration::from_millis(520),
                    |el, t| {
                        // A spring's overshoot, roughly: up past its place and settling back.
                        let s = 1.0 - (1.0 - t).powi(3) * (1.0 + 2.6 * t);
                        el.mt(px(14.0 * (1.0 - s))).opacity(t.min(1.0) * 0.6 + 0.4 * s.clamp(0.0, 1.0))
                    },
                ))
                .when(!eyebrow.is_empty(), |el| {
                    el.child(
                        div()
                            .mt(px(6.0))
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::EXTRA_BOLD)
                            .text_color(accent(server))
                            .child(eyebrow.to_uppercase()),
                    )
                })
                .child(div().text_xl().font_weight(gpui_kit::FontWeight::EXTRA_BOLD).child(server.name.clone()))
                .when(members > 0, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(crate::ui::widgets::icon("users").size(px(14.0)))
                            .child(if members == 1 { "1 member".to_owned() } else { format!("{members} members") }),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hues_match_the_web() {
        // hueOf("abc") in the web app: ((97*31+98)*31+99) % 360.
        assert_eq!(hue_of("abc"), ((97 * 31 + 98) * 31 + 99) % 360);
    }

    #[test]
    fn light_accents_get_dark_text() {
        assert_eq!(on_accent(hsla(0.12, 0.9, 0.85, 1.0)).l, 0.12);
        assert_eq!(on_accent(hsla(0.9, 0.7, 0.58, 1.0)), gpui_kit::white());
    }

    #[test]
    fn hex_both_ways() {
        assert_eq!(parse_hex("#ff8800"), Some(0xff8800));
        assert_eq!(parse_hex("f80"), Some(0xff8800));
        assert_eq!(parse_hex(" #ABCDEF "), Some(0xabcdef));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("zzz"), None);
        assert_eq!(hex(0x0a0b0c), "#0a0b0c");
    }

    #[test]
    fn covering_keeps_the_focus_in_view() {
        // A 1000×400 picture in a 400×400 box: scaled to 1000×400, so it slides sideways only.
        let (l, t, w, h) = cover(1000.0, 400.0, 400.0, 400.0, (0, 50));
        assert_eq!((l, t, w, h), (0.0, 0.0, 1000.0, 400.0));
        let (l, ..) = cover(1000.0, 400.0, 400.0, 400.0, (100, 50));
        assert_eq!(l, -600.0);
        let (l, ..) = cover(1000.0, 400.0, 400.0, 400.0, (50, 50));
        assert_eq!(l, -300.0);
    }
}
