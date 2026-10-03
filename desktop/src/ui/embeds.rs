//! Rich cards under a message, as apps post them through webhooks: a colored
//! edge, a title that can link somewhere, Markdown, fields side by side, a
//! thumbnail and a big picture. Like the web app's `chat/Embeds.tsx`.

use std::time::Duration;

use gpui_kit::component::text::TextView;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, FontWeight, Hsla, IntoElement, ObjectFit, ParentElement as _, SharedString, Styled as _,
    StyledImage as _, div, img, px, rgb,
};

use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha};

/// Only links a browser should open: http(s).
fn safe(url: &str) -> Option<&str> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://")).then_some(url)
}

fn markdown(id: String, text: &str) -> TextView {
    crate::ui::text::markdown(SharedString::from(id), crate::ui::text::images_as_links(text))
        .markdown_extensions(crate::ui::emoji::markdown_extensions())
        .selectable(true)
}

pub fn embeds(message: &str, embeds: &[pb::Embed], p: &Palette) -> AnyElement {
    let mut list = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    for (n, embed) in embeds.iter().enumerate() {
        list = list.child(motion::rise(
            card(&format!("{message}|{n}"), embed, p),
            SharedString::from(format!("embed|{message}|{n}")),
            Duration::from_millis(60 * n as u64),
            6.0,
        ));
    }
    list.into_any_element()
}

fn card(id: &str, embed: &pb::Embed, p: &Palette) -> gpui_kit::Div {
    let edge: Hsla = if embed.color != 0 { rgb(embed.color as u32).into() } else { p.border.into() };
    let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(4.0));
    if !embed.title.is_empty() {
        let title = match safe(&embed.url) {
            Some(url) => format!("[{}]({url})", embed.title.replace(['[', ']'], "")),
            None => embed.title.clone(),
        };
        text = text.child(
            div()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.primary)
                .child(markdown(format!("embed-title|{id}"), &title)),
        );
    }
    if !embed.description.is_empty() {
        text = text.child(div().text_sm().child(markdown(format!("embed-body|{id}"), &embed.description)));
    }
    if !embed.fields.is_empty() {
        let mut fields = div().mt(px(4.0)).flex().flex_wrap().gap_y(px(8.0));
        for (n, field) in embed.fields.iter().enumerate() {
            fields = fields.child(
                div()
                    .min_w_0()
                    .pr(px(16.0))
                    .map(|el| if field.inline { el.w(gpui_kit::relative(0.33)) } else { el.w_full() })
                    .child(div().text_xs().font_weight(FontWeight::EXTRA_BOLD).child(field.name.clone()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(markdown(format!("embed-field|{id}|{n}"), &field.value)),
                    ),
            );
        }
        text = text.child(fields);
    }
    let thumbnail = safe(&embed.thumbnail_url).map(|url| {
        img(SharedString::from(url.to_owned()))
            .size(px(64.0))
            .flex_none()
            .rounded(px(10.0))
            .object_fit(ObjectFit::Cover)
    });
    let image = safe(&embed.image_url).map(|url| {
        img(SharedString::from(url.to_owned()))
            .w_full()
            .max_h(px(288.0))
            .mt(px(8.0))
            .rounded(px(10.0))
            .object_fit(ObjectFit::Cover)
    });
    div()
        .max_w(px(520.0))
        .flex()
        .rounded(px(12.0))
        .overflow_hidden()
        .border_1()
        .border_color(p.border)
        .bg(alpha(p.card, 0.7))
        .child(div().w(px(4.0)).flex_none().bg(edge))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p(px(12.0))
                .child(div().flex().gap(px(12.0)).child(text).when_some(thumbnail, |el, t| el.child(t)))
                .when_some(image, |el, i| el.child(i)),
        )
}
