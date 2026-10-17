//! A gacha pull shared in chat (`Message.gacha`, docs/economy.md), drawn like
//! the web's `components/chat/GachaShareCard.tsx`: the pull's cards as the
//! server kept them, the rarest glowing, and a button to save a picture of
//! them (`core::gacha_share`). A profile item the server still has shows
//! itself, worn by whoever pulled it: a decoration over their avatar, an
//! effect's still on a tiny card of theirs.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Rgba, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, div, linear_color_stop, linear_gradient, px,
};

use crate::core::gacha_share::{self as gs, Chip, ImageLabels, ShareCards};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::profile_effects::{self as fx, Particle};
use crate::core::profile_items::item_effect;
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::profile_effect::{mini_card, still_effect};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, decorated, icon};

const NIGHT: u32 = 0x0b0a14;
const FACE: u32 = 0x15131f;

fn rgb(c: u32) -> Rgba {
    gpui_kit::rgb(c)
}

/// How wide the tiny card under a shared effect is.
const EFFECT_CARD: f32 = 29.0;

/// A shared pull as a message draws it: its cards, and the item each one won.
pub struct SharedPull {
    pub cards: ShareCards,
    worn: Vec<Option<Worn>>,
}

/// What a card won, as it's worn.
enum Worn {
    /// A decoration's picture.
    Decoration(String),
    /// An effect, and its particles planned once for its still.
    Effect(fx::Spec, Vec<Particle>),
}

impl SharedPull {
    /// The pull a message shares, each card's item found among its server's `items`; `seed`
    /// is whoever pulled it.
    pub fn new(share: &pb::GachaShare, items: &[pb::ProfileItem], seed: &str) -> Self {
        let worn = share
            .cards
            .iter()
            .map(|c| {
                let item = items.iter().find(|i| !c.item_id.is_empty() && i.id == c.item_id)?;
                if item.kind == pb::ProfileItemKind::Decoration as i32 {
                    return (!item.picture_url.is_empty()).then(|| Worn::Decoration(item.picture_url.clone()));
                }
                let spec = item_effect(item)?;
                let particles = fx::plan(&spec, EFFECT_CARD, EFFECT_CARD * 5.0 / 4.0, seed);
                Some(Worn::Effect(spec, particles))
            })
            .collect();
        Self { cards: gs::cards_of_share(share), worn }
    }

    /// What of `items` the cards would draw, for knowing when a message must be drawn again.
    pub fn digest(share: &pb::GachaShare, items: &[pb::ProfileItem], h: &mut impl std::hash::Hasher) {
        use std::hash::Hash as _;
        (&share.pull_id, share.cards.len()).hash(h);
        for c in &share.cards {
            items
                .iter()
                .find(|i| !c.item_id.is_empty() && i.id == c.item_id)
                .map(|i| (&i.picture_url, &i.effect))
                .hash(h);
        }
    }
}

/// A card's item as it's worn by `author`, or None to draw its sparkle.
fn worn(worn: Option<&Worn>, author: Option<&pb::User>, p: &Palette) -> Option<AnyElement> {
    let seed = author.map_or("?", |u| u.id.as_str());
    match worn? {
        Worn::Decoration(url) => {
            Some(div().mt(px(9.0)).child(decorated(avatar(author, 26.0, p), 26.0, Some(url))).into_any_element())
        }
        Worn::Effect(spec, particles) => Some(
            div()
                .mt(px(4.0))
                .child(mini_card(seed, -1, EFFECT_CARD, p).child(still_effect(spec, particles, seed, -1, p)))
                .into_any_element(),
        ),
    }
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
pub fn share_card(
    mid: &str,
    pull: &Rc<SharedPull>,
    author: &str,
    user: Option<&pb::User>,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
) -> AnyElement {
    let cards = &pull.cards;
    let glow = rgb(gs::best_card(&cards.cards).map_or(0xf59e0b, |c| c.color));
    let save = {
        let (pull, author, this) = (pull.clone(), author.to_owned(), this.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let task = save_image(&pull.cards, &author, cx);
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
    for (n, card) in cards.cards.iter().enumerate() {
        let color = rgb(card.color);
        let sparkle = || {
            div()
                .mt(px(6.0))
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(color, 0.22))
                .text_color(color)
                .child(icon("sparkles").size(px(if card.rarity >= 2 { 16.0 } else { 14.0 })))
                .into_any_element()
        };
        let art = worn(pull.worn.get(n).and_then(Option::as_ref), user, p).unwrap_or_else(sparkle);
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
                        .child(art)
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
