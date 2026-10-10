//! Rich cards under a message, as apps post them through webhooks: a colored
//! edge, a title that can link somewhere, Markdown, fields side by side, a
//! thumbnail and a big picture. Like the web app's `chat/Embeds.tsx`.

use std::time::Duration;

use gpui_kit::component::text::TextView;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, FontWeight, Hsla, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, SpringConfig, Stateful, Styled as _, StyledImage as _, div, img, px, rgb,
    sampled_easing,
};

use crate::pb;
use crate::ui::motion;
use crate::ui::theme::Palette;

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

/// The embeds under a message. `animate` for a message that just came in,
/// whose cards slide in from the left one after another, each lit for a
/// moment by a wash of its colour (the web's `animate`); others just show.
pub fn embeds(message: &str, embeds: &[pb::Embed], p: &Palette, animate: bool) -> AnyElement {
    let mut list = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    for (n, embed) in embeds.iter().enumerate() {
        let card = card(&format!("{message}|{n}"), embed, p, animate);
        list = list.child(if animate {
            slide_in(
                card,
                SharedString::from(format!("embed|{message}|{n}")),
                Duration::from_millis(80 + 60 * n as u64),
            )
        } else {
            card.into_any_element()
        });
    }
    list.into_any_element()
}

/// A card coming in after `delay`: a fade while it slides 10px from the left
/// and grows from 98%, on the web's `stiffness: 460, damping: 30`.
fn slide_in(card: Stateful<gpui_kit::Div>, id: SharedString, delay: Duration) -> AnyElement {
    let (duration, easing) = sampled_easing(SpringConfig::new(460.0, 30.0, 1.0), 0.002);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32();
    card.with_animation(
        id,
        Animation::new(total).with_easing(move |t| if t <= start { 0.0 } else { easing((t - start) / (1.0 - start)) }),
        |el, t| el.opacity(t.clamp(0.0, 1.0)).translate_x(px((t - 1.0) * 10.0)).scale(0.98 + 0.02 * t),
    )
    .into_any_element()
}

fn card(id: &str, embed: &pb::Embed, p: &Palette, animate: bool) -> Stateful<gpui_kit::Div> {
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
        text = text.child(
            div()
                .text_size(px(13.3))
                .line_height(px(21.6))
                .child(markdown(format!("embed-body|{id}"), &embed.description)),
        );
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
                            .text_size(px(12.6))
                            .text_color(p.muted_foreground)
                            .child(markdown(format!("embed-field|{id}|{n}"), &field.value)),
                    ),
            );
        }
        text = text.child(fields);
    }
    let thumbnail = safe(&embed.thumbnail_url).map(|url| {
        crate::ui::widgets::picture(url.to_owned())
            .size(px(64.0))
            .flex_none()
            .rounded(crate::ui::theme::radius_lg())
            .object_fit(ObjectFit::Cover)
    });
    let image = safe(&embed.image_url).map(|url| {
        crate::ui::widgets::picture(url.to_owned())
            .w_full()
            .max_h(px(288.0))
            .mt(px(8.0))
            .rounded(crate::ui::theme::radius_lg())
            .object_fit(ObjectFit::Cover)
    });
    // The web's card: a 4px coloured edge, `rounded-xl`, the card at 70% (drawn
    // solid, since GPUI would draw the shadow through a see-through fill).
    let radius = crate::ui::theme::radius_xl();
    // A new card's colour washes across it from the edge and fades (1.2s).
    let wash = animate.then(|| {
        let (from, to) = (Hsla { a: edge.a * 0.35, ..edge }, Hsla { a: 0.0, ..edge });
        motion::once(
            div().absolute().inset_0().rounded(radius).bg(gpui_kit::linear_gradient(
                90.0,
                gpui_kit::linear_color_stop(from, 0.0),
                gpui_kit::linear_color_stop(to, 0.7),
            )),
            SharedString::from(format!("embed-wash|{id}")),
            Duration::from_millis(1200),
            |el, t| el.opacity(1.0 - t),
        )
    });
    // It lifts a pixel under the pointer (`whileHover={{ y: -1 }}`).
    div()
        .id(SharedString::from(format!("embed-card|{id}")))
        .relative()
        .hover(|s| s.translate_y(px(-1.0)))
        .max_w(px(512.0))
        .flex()
        .rounded(radius)
        .bg(edge)
        .pl(px(4.0))
        .text_sm()
        .line_height(px(20.0))
        .shadow(crate::ui::polls::shadow_sm())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .rounded_r(radius)
                .rounded_l(px((f32::from(radius) - 4.0).max(0.0)))
                .border_1()
                .border_l_0()
                .border_color(p.border)
                .bg(crate::ui::theme::mix(p.chat_surface.into(), p.card, 0.7))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .p(px(12.0))
                        .child(div().flex().gap(px(12.0)).child(text).when_some(thumbnail, |el, t| el.child(t)))
                        .when_some(image, |el, i| el.child(i)),
                ),
        )
        .children(wash)
}
