//! A gacha pull shared in chat (`Message.gacha`, docs/economy.md), drawn like
//! the web's `components/chat/GachaShareCard.tsx`: the pull's cards as the
//! server kept them, the rarest glowing, and a button to save a picture of
//! them (`core::gacha_share`).

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Rgba, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, div, linear_color_stop, linear_gradient, px,
};

use crate::core::gacha_share::{self as gs, Chip, ImageLabels, ShareCards};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::app::FuwaApp;
use crate::ui::theme::{alpha, radius_2xl, radius_xl};
use crate::ui::widgets::icon;

const NIGHT: u32 = 0x0b0a14;
const FACE: u32 = 0x15131f;

fn rgb(c: u32) -> Rgba {
    gpui_kit::rgb(c)
}

/// What a card's chip says, in the app's language.
pub fn chip_words(chip: Chip) -> String {
    t(match chip {
        Chip::Refunded => "chattools.economy.gacha.refunded",
        Chip::Duplicate => "chattools.economy.gacha.duplicate",
        Chip::Guaranteed => "chattools.economy.gacha.guaranteed",
    })
}

/// Asks where, then saves a picture of a pull's cards there. Comes back with
/// what went wrong, if anything did (nothing when it's saved or the person
/// changed their mind).
pub fn save_image(cards: &ShareCards, who: &str, cx: &mut App) -> Task<Option<String>> {
    let subtitle = t_with(
        "chattools.economy.gacha.imageSubtitle",
        &[("name", Arg::Str(who)), ("count", Arg::Num(cards.cards.len() as i64))],
    );
    let image = gs::share_image(cards, &ImageLabels { subtitle, chip: &chip_words });
    let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
    let path = cx.prompt_for_new_path(&dir, Some(&gs::image_name(&cards.banner_name)));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = path.await else { return None };
        let written = cx
            .background_executor()
            .spawn(async move {
                gs::render_png(&image, 2.0).and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()))
            })
            .await;
        written.err().map(|_| t("chattools.economy.gacha.imageFailed"))
    })
}

/// The card under a message that shares a pull.
pub fn share_card(mid: &str, cards: &Rc<ShareCards>, author: &str, this: &WeakEntity<FuwaApp>) -> AnyElement {
    let glow = rgb(gs::best_card(&cards.cards).map_or(0xf59e0b, |c| c.color));
    let save = {
        let (cards, author, this) = (cards.clone(), author.to_owned(), this.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let task = save_image(&cards, &author, cx);
            let this = this.clone();
            cx.spawn(async move |cx| {
                if let Some(why) = task.await {
                    let _ = this.update(cx, |this, cx| this.toast("image", why, String::new(), None, None, cx));
                }
            })
            .detach();
        }
    };
    let header = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .mb(px(10.0))
        .child(icon("dices").size(px(16.0)).text_color(glow))
        .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(t_with(
            "chattools.economy.gacha.sharedHeader",
            &[("count", Arg::Num(cards.cards.len() as i64)), ("banner", Arg::Str(&cards.banner_name))],
        )))
        .child(
            div()
                .id(SharedString::from(format!("gacha-share-save|{mid}")))
                .flex_none()
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(rgb(0xffffff), 0.1))
                .hover(|s| s.bg(alpha(rgb(0xffffff), 0.2)))
                .cursor_pointer()
                .tooltip(|window, cx| {
                    crate::ui::overlay::Tip::new(t("chattools.economy.gacha.saveImage")).build(window, cx)
                })
                .on_click(save)
                .child(icon("download").size(px(14.0))),
        );
    let mut grid = div().flex().flex_wrap().gap(px(6.0));
    for card in &cards.cards {
        let color = rgb(card.color);
        let chip = gs::chip(card);
        grid = grid.child(
            div()
                .relative()
                .flex_none()
                .w(px(84.0))
                .h(px(112.0))
                .rounded(radius_xl())
                .border_2()
                .border_color(color)
                .bg(rgb(FACE))
                .overflow_hidden()
                .when(card.rarity >= 3, |el| {
                    el.shadow(vec![gpui_kit::BoxShadow {
                        color: alpha(color, 0.65),
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(14.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                })
                .child(div().absolute().inset_0().bg(linear_gradient(
                    180.0,
                    linear_color_stop(alpha(color, 0.25), 0.0),
                    linear_color_stop(alpha(color, 0.0), 0.6),
                )))
                .child(
                    div()
                        .relative()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .px(px(4.0))
                        .pt(px(6.0))
                        .pb(px(4.0))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .line_height(px(11.0))
                                .text_color(color)
                                .child("★".repeat(card.stars)),
                        )
                        .child(
                            div()
                                .mt(px(6.0))
                                .size(px(28.0))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(alpha(color, 0.22))
                                .text_color(color)
                                .child(icon("sparkles").size(px(if card.rarity >= 2 { 16.0 } else { 14.0 }))),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .w_full()
                                .text_center()
                                .text_size(px(11.0))
                                .line_height(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .line_clamp(2)
                                .child(card.prize_name.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_center()
                                .truncate()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .text_color(color)
                                .child(card.tier_name.to_uppercase()),
                        )
                        .when_some(chip, |el, chip| {
                            el.child(
                                div()
                                    .w_full()
                                    .text_center()
                                    .truncate()
                                    .text_size(px(9.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(alpha(rgb(0xffffff), 0.6))
                                    .child(chip_words(chip)),
                            )
                        }),
                ),
        );
    }
    div()
        .mt(px(4.0))
        .max_w(px(480.0))
        .relative()
        .overflow_hidden()
        .rounded(radius_2xl())
        .bg(rgb(NIGHT))
        .text_color(rgb(0xffffff))
        .child(div().absolute().inset_0().bg(linear_gradient(
            180.0,
            linear_color_stop(alpha(glow, 0.33), 0.0),
            linear_color_stop(alpha(glow, 0.0), 0.7),
        )))
        .child(div().relative().p(px(12.0)).child(header).child(grid))
        .into_any_element()
}
