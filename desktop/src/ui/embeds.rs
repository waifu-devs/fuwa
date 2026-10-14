//! Rich cards under a message, as apps post them through webhooks: a colored
//! edge, a title that can link somewhere, Markdown, fields side by side, a
//! thumbnail and a big picture; and the plainer cards the instance makes
//! for links in a message (link previews, docs/link-previews.md), whose words
//! are plain text. Like the web app's `chat/Embeds.tsx`.

use std::time::Duration;

use gpui_kit::component::text::TextView;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, SharedString, SpringConfig, Stateful, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, WeakEntity, div, px, rgb, sampled_easing,
};

use crate::core::i18n::t;
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
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

/// A card's title: its link always asks before opening, since the app chose the words.
fn title_markdown(id: String, text: &str) -> TextView {
    crate::ui::text::markdown_masked(SharedString::from(id), crate::ui::text::images_as_links(text))
        .markdown_extensions(crate::ui::emoji::markdown_extensions())
        .selectable(true)
}

/// The embeds under a message. `animate` for a message that just came in,
/// whose cards slide in from the left one after another, each lit for a
/// moment by a wash of its colour (the web's `animate`); others just show.
/// Link previews rise in instead, and `hide` (your own messages) gives them
/// the × that takes them off.
pub fn embeds(
    message: &str,
    embeds: &[pb::Embed],
    p: &Palette,
    animate: bool,
    hide: Option<&WeakEntity<FuwaApp>>,
) -> AnyElement {
    let mut list = div().mt(px(4.0)).flex().flex_col().gap(px(6.0));
    for (n, embed) in embeds.iter().enumerate() {
        let id = format!("{message}|{n}");
        let (card, from) = if embed.preview {
            let Some(card) = preview(&id, message, embed, p, hide) else { continue };
            (card, (0.0, 6.0))
        } else {
            (card(&id, embed, p, animate), (-10.0, 0.0))
        };
        list = list.child(if animate {
            slide_in(card, SharedString::from(format!("embed|{id}")), Duration::from_millis(80 + 60 * n as u64), from)
        } else {
            card.into_any_element()
        });
    }
    list.into_any_element()
}

/// A card coming in after `delay`: a fade while it moves in from `from` (x,
/// y) and grows from 98%, on the web's `stiffness: 460, damping: 30`.
fn slide_in(card: Stateful<gpui_kit::Div>, id: SharedString, delay: Duration, from: (f32, f32)) -> AnyElement {
    let (duration, easing) = sampled_easing(SpringConfig::new(460.0, 30.0, 1.0), 0.002);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32();
    card.with_animation(
        id,
        Animation::new(total).with_easing(move |t| if t <= start { 0.0 } else { easing((t - start) / (1.0 - start)) }),
        move |el, t| {
            el.opacity(t.clamp(0.0, 1.0))
                .translate_x(px((1.0 - t) * from.0))
                .translate_y(px((1.0 - t) * from.1))
                .scale(0.98 + 0.02 * t)
        },
    )
    .into_any_element()
}

/// A picture that opens `url` when clicked.
fn picture_link(id: String, picture: impl IntoElement, url: &str) -> Stateful<gpui_kit::Div> {
    let url = url.to_owned();
    div().id(SharedString::from(id)).cursor_pointer().on_click(move |_, _, cx| cx.open_url(&url)).child(picture)
}

/// A link preview (the web's `LinkPreview`): the site small on top, the
/// page's title as the link, its description as plain text, a thumbnail at
/// the side or a big picture under them; just the picture when the page has
/// no words. Nothing when its link isn't one a browser should open.
fn preview(
    id: &str,
    message: &str,
    embed: &pb::Embed,
    p: &Palette,
    hide: Option<&WeakEntity<FuwaApp>>,
) -> Option<Stateful<gpui_kit::Div>> {
    let url = safe(&embed.url)?;
    let words = !embed.title.is_empty() || !embed.description.is_empty();
    let image = safe(&embed.image_url);
    if !words && image.is_none() {
        return None;
    }
    let group = SharedString::from(format!("preview|{id}"));
    let radius = crate::ui::theme::radius_xl();
    // The × on your own message, shown while the card is pointed at.
    let hide = hide.map(|this| {
        let (this, message) = (this.clone(), message.to_owned());
        let label = t("chat.messages.hidePreviews");
        let (fg, back) = (p.foreground, alpha(p.background, 0.8));
        div()
            .id(SharedString::from(format!("preview-hide|{id}")))
            .absolute()
            .top(px(6.0))
            .right(px(6.0))
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(back)
            .text_color(p.muted_foreground)
            .shadow(crate::ui::polls::shadow_sm())
            .cursor_pointer()
            .opacity(0.0)
            .group_hover(group.clone(), |s| s.opacity(1.0))
            .hover(move |s| s.text_color(fg).scale(1.1))
            .active(|s| s.scale(0.9))
            .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let message = message.clone();
                let _ = this.update(cx, |this, cx| this.hide_link_previews(message, cx));
            })
            .child(crate::ui::widgets::icon("x").size(px(14.0)))
    });
    let card = div().id(SharedString::from(format!("embed-card|{id}"))).group(group).relative().text_sm();
    if !words {
        // Only a picture: itself, linking to the page, at its own size but no
        // wider than the card or taller than 320px (the web's `max-h-80
        // max-w-full`). Rows, so neither it nor its link stretches wider than
        // it is, and both may shrink to the card.
        let picture = crate::ui::widgets::picture(image?.to_owned())
            .min_w_0()
            .max_h(px(320.0))
            .rounded(radius)
            .object_fit(ObjectFit::Contain);
        let link = picture_link(format!("preview-picture|{id}"), picture, url).flex().items_start().min_w_0();
        return Some(card.max_w(px(448.0)).flex().items_start().child(link).children(hide));
    }
    let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(2.0));
    if !embed.site_name.is_empty() {
        text = text.child(div().truncate().text_xs().text_color(p.muted_foreground).child(embed.site_name.clone()));
    }
    if !embed.title.is_empty() {
        let link = url.to_owned();
        text = text.child(
            div()
                .id(SharedString::from(format!("preview-title|{id}")))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.primary)
                .line_clamp(2)
                .text_ellipsis()
                .cursor_pointer()
                .hover(|s| s.underline())
                .on_click(move |_, _, cx| cx.open_url(&link))
                .child(embed.title.clone()),
        );
    }
    if !embed.description.is_empty() {
        // Plain text, never Markdown: the page wrote it, not the author.
        text = text.child(
            div()
                .text_size(px(12.6))
                .text_color(p.muted_foreground)
                .line_clamp(3)
                .text_ellipsis()
                .child(embed.description.clone()),
        );
    }
    let thumbnail = safe(&embed.thumbnail_url).map(|url| {
        crate::ui::widgets::picture(url.to_owned())
            .size(px(64.0))
            .flex_none()
            .rounded(crate::ui::theme::radius_lg())
            .object_fit(ObjectFit::Cover)
    });
    let image = image.map(|image| {
        let picture = crate::ui::widgets::picture(image.to_owned())
            .w_full()
            .max_h(px(288.0))
            .rounded(crate::ui::theme::radius_lg())
            .object_fit(ObjectFit::Cover);
        picture_link(format!("preview-picture|{id}"), picture, url).mt(px(8.0))
    });
    let edge: Hsla = if embed.color != 0 { rgb(embed.color as u32).into() } else { p.border.into() };
    // The webhook card's shape (a 4px edge, `rounded-xl`, the card at 70%), plainer: no wash, no lift.
    Some(
        card.max_w(px(448.0))
            .flex()
            .rounded(radius)
            .bg(edge)
            .pl(px(4.0))
            .line_height(px(20.0))
            .shadow(crate::ui::polls::shadow_sm())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded_r(radius)
                    .rounded_l(px((f32::from(radius) - 4.0).max(0.0)))
                    .border_1()
                    .border_l_0()
                    .border_color(p.border)
                    .bg(crate::ui::theme::mix(p.chat_surface.into(), p.card, 0.7))
                    .p(px(12.0))
                    .child(div().flex().gap(px(12.0)).child(text).when_some(thumbnail, |el, t| el.child(t)))
                    .when_some(image, |el, i| el.child(i)),
            )
            .children(hide),
    )
}

impl FuwaApp {
    /// Takes the link previews off one of your own messages, for good.
    pub fn hide_link_previews(&mut self, message_id: String, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.hide_link_previews(&key, &server, &channel, &message_id).await },
            |this, result, cx| {
                if let Err(err) = result {
                    this.toast("circle-alert", t("chat.messages.hidePreviews"), err.message, None, None, cx);
                }
                this.sync_list(cx);
                cx.notify();
            },
        );
    }
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
                .child(title_markdown(format!("embed-title|{id}"), &title)),
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
