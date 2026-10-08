//! Small pieces the screens share: icons, avatars, server icons, badges,
//! buttons and fields, each with its hover and press.

use gpui_kit::{
    AnyElement, App, Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Rgba, SharedString, Stateful, StatefulInteractiveElement as _, Styled, StyledImage as _, div,
    hsla, img, px, svg,
};

use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;

use crate::core::store::Connection;
use crate::pb;
use crate::ui::theme::{self, Palette, alpha, corner, mix};

/// A Lucide icon by name (`hash`, `plus`, `settings`...).
/// It takes the color of the text around it unless given one.
pub fn icon(name: &str) -> Icon {
    Icon::default().path(SharedString::from(format!("icons/{name}.svg"))).flex_none()
}

/// fuwa's little cloud, drawn in two layers (the cloud, then its face).
pub fn fuwa_mark(size: f32, p: &Palette) -> impl IntoElement {
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .child(svg().path("fuwa/mark.svg").absolute().inset_0().size(px(size)).text_color(p.primary))
        .child(svg().path("fuwa/face.svg").absolute().inset_0().size(px(size)).text_color(p.primary_foreground))
}

/// The hue that belongs to an id, as the web app's `hueOf`.
pub fn hue_of(id: &str) -> f32 {
    let mut h: u32 = 0;
    for c in id.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(c));
    }
    (h % 360) as f32
}

/// The web's `.server-gradient` behind initials: a 135° sweep from the id's
/// hue to 40° on, under a soft light toward the top left (the web's radial
/// highlight, drawn as a second sweep since GPUI has no radial gradients).
pub fn hue_gradient(id: &str, radius: f32, el: Div) -> Div {
    let h = hue_of(id);
    let at = |deg: f32, s: f32, l: f32, a: f32| hsla((deg % 360.0) / 360.0, s, l, a);
    el.bg(gpui_kit::linear_gradient(
        135.0,
        gpui_kit::linear_color_stop(at(h, 0.7, 0.55, 1.0), 0.0),
        gpui_kit::linear_color_stop(at(h + 40.0, 0.7, 0.45, 1.0), 1.0),
    ))
    .child(div().absolute().inset_0().rounded(px(radius)).bg(gpui_kit::linear_gradient(
        150.0,
        gpui_kit::linear_color_stop(at(h, 0.9, 0.75, 0.75), 0.0),
        gpui_kit::linear_color_stop(at(h, 0.9, 0.75, 0.0), 0.5),
    )))
}

/// The web's `.name-tint`: someone's own hue (75% saturation, 55% light)
/// mixed 70/30 with the text color, for names without a role color.
pub fn name_tint(id: &str, p: &Palette) -> Hsla {
    let hue: Rgba = hsla(hue_of(id) / 360.0, 0.75, 0.55, 1.0).into();
    crate::ui::theme::mix(p.foreground, hue, 0.7)
}

/// "Mika Sato" → "MS", as the web app's `initials`.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let pick: Vec<&str> = match words.as_slice() {
        [] => return "?".into(),
        [one] => vec![one],
        [first, .., last] => vec![first, last],
    };
    pick.iter().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect()
}

/// Someone's picture, or their initial on their color.
pub fn avatar(user: Option<&pb::User>, size: f32, _p: &Palette) -> Div {
    let (id, name, url) = match user {
        Some(u) => (u.id.as_str(), crate::core::store::user_name(u), u.avatar_url.as_str()),
        None => ("?", "?".to_owned(), ""),
    };
    let base = div().size(px(size)).flex_none().rounded_full().overflow_hidden();
    let letter: String = initials(&name).chars().take(1).collect();
    let id = id.to_owned();
    let fallback = move || {
        hue_gradient(&id, size / 2.0, div().relative().size_full().rounded_full().overflow_hidden())
            .flex()
            .items_center()
            .justify_center()
            .text_color(gpui_kit::white())
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(size * 0.35))
            .child(div().relative().child(letter.clone()))
            .into_any_element()
    };
    if url.is_empty() {
        base.child(fallback())
    } else {
        let fallback2 = fallback.clone();
        base.child(
            img(SharedString::from(url.to_owned()))
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(fallback)
                .with_loading(fallback2),
        )
    }
}

/// An avatar with the decoration someone wears (docs/profile-items.md): its
/// picture centred over the avatar at 1.2 times its size, catching nothing.
/// The picture is always an upload on the instance (`InstanceState::decoration_url`).
pub fn decorated(avatar: Div, size: f32, decoration: Option<&str>) -> Div {
    let el = div().relative().size(px(size)).flex_none().child(avatar);
    match decoration.filter(|url| !url.is_empty()) {
        None => el,
        Some(url) => el.child(
            img(SharedString::from(url.to_owned()))
                .absolute()
                .left(px(-size * 0.1))
                .top(px(-size * 0.1))
                .size(px(size * 1.2))
                .object_fit(ObjectFit::Contain),
        ),
    }
}

/// A server's icon: its picture, or its initials on its color. Round until
/// it's open or hovered, then a rounded square, like Discord's.
pub fn server_icon(server: &pb::Server, size: f32, radius: f32, _p: &Palette) -> Div {
    let text = initials(&server.name);
    let base = div().size(px(size)).flex_none().rounded(px(radius)).overflow_hidden();
    let id = server.id.clone();
    let fallback = move || {
        hue_gradient(&id, radius, div().relative().size_full().rounded(px(radius)).overflow_hidden())
            .flex()
            .items_center()
            .justify_center()
            .text_color(gpui_kit::white())
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(size / 3.0))
            .child(div().relative().child(text.clone()))
            .into_any_element()
    };
    if server.icon_url.is_empty() {
        base.child(fallback())
    } else {
        let fallback2 = fallback.clone();
        base.child(
            img(SharedString::from(server.icon_url.clone()))
                .size_full()
                .rounded(px(radius))
                .object_fit(ObjectFit::Cover)
                .with_fallback(fallback)
                .with_loading(fallback2),
        )
    }
}

/// A red count, like on Discord's rail.
pub fn badge(count: u32, p: &Palette) -> Div {
    let text = if count > 99 { "99+".to_owned() } else { count.to_string() };
    div()
        .h(px(18.0))
        .min_w(px(18.0))
        .px(px(5.0))
        .rounded_full()
        .bg(p.destructive)
        .border_2()
        .border_color(p.rail)
        .flex()
        .items_center()
        .justify_center()
        .text_color(gpui_kit::white())
        .text_size(px(10.5))
        .font_weight(FontWeight::EXTRA_BOLD)
        .child(text)
}

/// The dot that says how a connection is doing.
pub fn conn_dot(connection: Connection, p: &Palette) -> Div {
    let color: Hsla = match connection {
        Connection::Live => gpui_kit::rgb(0x3ecf8e).into(),
        Connection::Connecting | Connection::Reconnecting => gpui_kit::rgb(0xf5a524).into(),
        Connection::Offline => p.destructive.into(),
        Connection::SignedOut => p.muted_foreground.into(),
    };
    // The web's .conn-dot (0.55rem) with its 2px ring in the rail's dark color.
    let ring = crate::ui::theme::mix(p.background, gpui_kit::rgb(0x000000), 0.25);
    div().size(px(12.8)).rounded_full().bg(color).border_2().border_color(ring)
}

/// A filled button in the primary color, with a glow on hover and a dip on press.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>, p: &Palette) -> Stateful<Div> {
    filled_button(id, label, p.primary, p.primary_foreground, p)
}

fn filled_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    color: Rgba,
    text: Rgba,
    p: &Palette,
) -> Stateful<Div> {
    let glow = alpha(color, 0.45);
    div()
        .id(id)
        .h(px(40.0))
        .px(px(18.0))
        .rounded(corner(12.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .bg(color)
        .text_color(text)
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .shadow(vec![gpui_kit::BoxShadow {
            color: alpha(color, 0.28),
            offset: gpui_kit::point(px(0.0), px(6.0)),
            blur_radius: px(18.0),
            spread_radius: px(-6.0),
            inset: false,
        }])
        .hover({
            let c = mix(color, p.foreground, 0.08);
            move |s| {
                s.bg(c).shadow(vec![gpui_kit::BoxShadow {
                    color: glow,
                    offset: gpui_kit::point(px(0.0), px(8.0)),
                    blur_radius: px(24.0),
                    spread_radius: px(-6.0),
                    inset: false,
                }])
            }
        })
        .active({
            let c = mix(color, p.foreground, 0.18);
            move |s| s.bg(c).top(px(1.0))
        })
        .child(label.into())
}

/// A quiet button: text on a soft background.
pub fn soft_button(id: impl Into<ElementId>, label: impl Into<SharedString>, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(36.0))
        .px(px(14.0))
        .rounded(corner(10.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .bg(p.secondary)
        .text_color(p.foreground)
        .font_weight(FontWeight::BOLD)
        .text_sm()
        .cursor_pointer()
        .hover({
            let c = mix(p.secondary, p.primary, 0.14);
            move |s| s.bg(c)
        })
        .active({
            let c = mix(p.secondary, p.primary, 0.24);
            move |s| s.bg(c).top(px(1.0))
        })
        .child(label.into())
}

/// A round icon button that lights up on hover.
pub fn icon_button(id: impl Into<ElementId>, name: &str, p: &Palette) -> Stateful<Div> {
    icon_button_in(id, name, p, p.primary)
}

/// An icon button that lights up in `color` when hovered, like red for delete.
pub fn icon_button_in(id: impl Into<ElementId>, name: &str, p: &Palette, color: Rgba) -> Stateful<Div> {
    let hover = alpha(color, 0.12);
    let fg = color;
    div()
        .id(id)
        .size(px(32.0))
        .flex_none()
        .rounded(corner(10.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(p.muted_foreground)
        .hover(move |s| s.bg(hover).text_color(fg))
        .active(move |s| s.top(px(1.0)))
        .child(icon(name).size(px(18.0)))
}

/// The web's round header button (`size-9 rounded-full`, a 20px icon):
/// muted, the muted fill on hover, and the primary at 10% while what it opens is open.
pub fn header_button(id: impl Into<ElementId>, name: &str, on: bool, p: &Palette) -> Stateful<Div> {
    let hover = p.muted;
    div()
        .id(id)
        .size(px(36.0))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(if on { p.primary } else { p.muted_foreground })
        .when(on, |el| el.bg(alpha(p.primary, 0.1)))
        .when(!on, |el| el.hover(move |s| s.bg(hover)))
        .child(icon(name).size(px(20.0)))
}

/// A composer tool (the web's `size-9 rounded-xl` with an 18px icon): muted,
/// the icon turns primary on hover, and it sits on the primary at 10% while open.
pub fn tool_button(id: impl Into<ElementId>, name: &str, open: bool, p: &Palette) -> Stateful<Div> {
    let fg = p.primary;
    div()
        .id(id)
        .size(px(36.0))
        .mb(px(2.0))
        .flex_none()
        .rounded(crate::ui::theme::radius_xl())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(if open { p.primary } else { p.muted_foreground })
        .when(open, |el| el.bg(alpha(p.primary, 0.1)))
        .hover(move |s| s.text_color(fg))
        .child(icon(name).size(px(18.0)))
}

/// Whether an account is an agent: one a program drives.
pub fn is_agent(user: Option<&pb::User>) -> bool {
    user.is_some_and(|u| u.kind == pb::AccountKind::Agent as i32)
}

/// The little tag on what isn't a person: "APP" for a webhook, "AGENT" (with
/// a robot) for an account a program drives, "BOT" for AutoMod. It pops in.
pub fn app_badge(id: impl Into<ElementId>, label: &'static str, p: &Palette) -> impl IntoElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let (duration, easing) = gpui_kit::sampled_easing(gpui_kit::SpringConfig::new(600.0, 18.0, 1.0), 0.002);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(2.0))
        .px(px(5.0))
        .h(px(15.0))
        .rounded(px(5.0))
        .bg(alpha(p.primary, 0.15))
        .text_color(p.primary)
        .text_size(px(10.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .when(label == "AGENT", |el| el.child(icon("bot").size(px(10.0))))
        .child(label)
        .with_animation(id, Animation::new(duration).with_easing(easing), |el, t| {
            el.opacity(t.clamp(0.0, 1.0)).mt(px((1.0 - t) * 3.0))
        })
}

/// A field with its label above it.
pub fn labeled(label: &str, field: impl IntoElement, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(label.to_uppercase()),
        )
        .child(field)
}

/// A soft card on the page.
pub fn card(p: &Palette) -> Div {
    div().bg(p.card).rounded(corner(20.0)).border_1().border_color(p.border).shadow(vec![gpui_kit::BoxShadow {
        color: alpha(p.primary, if p.dark { 0.18 } else { 0.14 }),
        offset: gpui_kit::point(px(0.0), px(24.0)),
        blur_radius: px(60.0),
        spread_radius: px(-28.0),
        inset: false,
    }])
}

/// An error line, when there is one.
pub fn error_line(error: Option<&str>, p: &Palette) -> Option<AnyElement> {
    let error = error?;
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(8.0))
            .rounded(corner(10.0))
            .bg(alpha(p.destructive, 0.12))
            .text_color(p.destructive)
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .child(icon("circle-alert").size(px(16.0)).text_color(p.destructive))
            .child(error.to_owned())
            .into_any_element(),
    )
}

/// The app's palette, for views.
pub fn pal(cx: &App) -> Palette {
    theme::palette(cx)
}
