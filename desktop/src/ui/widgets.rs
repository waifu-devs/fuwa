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

/// A stable color for something, from its id, as the web app's `hueOf`.
pub fn hue_color(id: &str, dark: bool) -> Hsla {
    let mut h: u32 = 0;
    for c in id.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u32::from(c));
    }
    hsla((h % 360) as f32 / 360.0, 0.62, if dark { 0.66 } else { 0.58 }, 1.0)
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
pub fn avatar(user: Option<&pb::User>, size: f32, p: &Palette) -> Div {
    let (id, name, url) = match user {
        Some(u) => (u.id.as_str(), crate::core::store::user_name(u), u.avatar_url.as_str()),
        None => ("?", "?".to_owned(), ""),
    };
    let base = div().size(px(size)).flex_none().rounded_full().overflow_hidden();
    let letter: String = initials(&name).chars().take(1).collect();
    let color = hue_color(id, p.dark);
    let fallback = move || {
        div()
            .size_full()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(color)
            .text_color(gpui_kit::white())
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(size * 0.42))
            .child(letter.clone())
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
pub fn server_icon(server: &pb::Server, size: f32, radius: f32, p: &Palette) -> Div {
    let color = hue_color(&server.id, p.dark);
    let text = initials(&server.name);
    let base = div().size(px(size)).flex_none().rounded(px(radius)).overflow_hidden();
    let fallback = move || {
        div()
            .size_full()
            .rounded(px(radius))
            .flex()
            .items_center()
            .justify_center()
            .bg(color)
            .text_color(gpui_kit::white())
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(size * if text.chars().count() > 1 { 0.34 } else { 0.42 }))
            .child(text.clone())
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
        Connection::Live => p.success.into(),
        Connection::Connecting | Connection::Reconnecting => hsla(0.12, 0.9, 0.55, 1.0),
        Connection::Offline => p.destructive.into(),
        Connection::SignedOut => p.muted_foreground.into(),
    };
    div().size(px(10.0)).rounded_full().bg(color).border_2().border_color(p.rail)
}

/// A filled button in the primary color, with a glow on hover and a dip on press.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>, p: &Palette) -> Stateful<Div> {
    filled_button(id, label, p.primary, p.primary_foreground, p)
}

/// The same in the destructive color, for deleting and other things that can't be undone.
pub fn danger_button(id: impl Into<ElementId>, label: impl Into<SharedString>, p: &Palette) -> Stateful<Div> {
    filled_button(id, label, p.destructive, gpui_kit::rgb(0xffffff), p)
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

/// A small heading over a group of things in a sidebar.
pub fn section_label(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .px(px(8.0))
        .pt(px(16.0))
        .pb(px(4.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(text.into().to_uppercase())
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
