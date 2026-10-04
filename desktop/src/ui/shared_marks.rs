//! The marks a shared channel carries wherever it shows: a badge in the
//! sidebar, a pill in the header, a note at its start, and a tag beside the
//! names of people from the other server. The web's `chat/Shared.tsx`.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Div, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha};
use crate::ui::widgets::server_icon;

/// Two linked rings: the app's sign for a channel shared between servers.
pub fn glyph(size: f32, color: impl Into<Hsla>) -> Div {
    let color = color.into();
    // The web's 16-unit drawing: rings of radius 3.75 at x 5.75 and 10.25.
    let unit = size / 16.0;
    let ring = |left: f32| {
        div()
            .absolute()
            .left(px(left * unit))
            .top(px(4.25 * unit))
            .size(px(7.5 * unit))
            .rounded_full()
            .map(|el| if size >= 20.0 { el.border_2() } else { el.border_1() })
            .border_color(color)
    };
    div().relative().flex_none().size(px(size)).child(ring(2.0)).child(ring(6.5))
}

/// A server on the other end of a shared channel, as a picture. Pictures only
/// ever come from this instance (it drops other instances'), and anything else
/// shows initials rather than reaching out somewhere new.
pub fn server_picture(server: &pb::SharedServer, instance_url: &str, size: f32, radius: f32, p: &Palette) -> Div {
    let here = instance_url.trim_end_matches('/');
    let ours = !here.is_empty()
        && (server.icon_url.starts_with(&format!("{here}/")) || server.icon_url.starts_with('/'))
        && !server.icon_url.starts_with("//");
    let icon_url = if ours && server.icon_url.starts_with('/') {
        format!("{here}{}", server.icon_url)
    } else if ours {
        server.icon_url.clone()
    } else {
        String::new()
    };
    let as_server = pb::Server { id: server.id.clone(), name: server.name.clone(), icon_url, ..Default::default() };
    server_icon(&as_server, size, radius, p)
}

/// Beside the name of someone from another server: which one, as a small muted chip.
pub fn server_tag(server: &pb::SharedServer, instance_url: &str, p: &Palette) -> Div {
    div()
        .flex_none()
        .max_w(px(160.0))
        .h(px(18.0))
        .pl(px(2.0))
        .pr(px(7.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded_full()
        .bg(alpha(p.muted, 0.9))
        .text_size(px(11.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(server_picture(server, instance_url, 14.0, 7.0, p))
        .child(div().min_w_0().truncate().child(server.name.clone()))
}

/// The header's pill: where a shared channel's messages live, or who it's shown to.
pub fn pill(text: String, id: &str, p: &Palette) -> impl IntoElement {
    motion::slide_in(
        div()
            .flex_none()
            .max_w(px(320.0))
            .h(px(24.0))
            .px(px(9.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded_full()
            .bg(alpha(p.primary, 0.12))
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(p.primary)
            .child(glyph(14.0, p.primary))
            .child(div().min_w_0().truncate().child(text)),
        SharedString::from(format!("shared-pill-{id}")),
        -6.0,
    )
}

/// The sidebar's mark beside a shared channel's name: pops up into place, and says who it's shared with.
pub fn badge(id: &str, label: String, color: impl Into<Hsla>) -> impl IntoElement {
    motion::rise(
        div()
            .id(SharedString::from(format!("shared-badge-{id}")))
            .flex_none()
            .size(px(18.0))
            .flex()
            .items_center()
            .justify_center()
            .child(glyph(13.0, color))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)),
        SharedString::from(format!("shared-badge-in-{id}")),
        Duration::from_millis(120),
        6.0,
    )
}
