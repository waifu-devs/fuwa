//! The shop's gacha (docs/economy.md), drawn like the web's
//! `components/economy/Gacha.tsx`: one banner at a time on a stage, its pack
//! floating over slow light rays, the top prizes, a pity meter and two big
//! Pull buttons, with the drop rates a click away. A pull plays out over the
//! whole window to the web's clock (`economy::reveal_plan`): the pack
//! charges in the color of the best thing inside, bursts, deals the cards
//! face down and turns them one by one, waiting a beat before a rare one,
//! with the web's sounds (`sounds::play_gacha`).

use std::f32::consts::{PI, TAU};
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Rgba,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, linear_color_stop,
    linear_gradient, px, radians, relative,
};

use super::{Facts, ShopView, countdown, empty};
use crate::core::dms::now_ms;
use crate::core::economy::{self as eco, RevealPlan, reveal};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::sounds::{self, GachaCue};
use crate::pb::{self, ShopListingKind as Kind};
use crate::ui::economy::{currency_icon, money};
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{icon, picture};

/// How wide each of a pull's `count` cards is in a window `w` wide.
fn card_width(count: usize, w: f32) -> f32 {
    if count == 1 {
        return 208.0;
    }
    let room = w.min(896.0 + 32.0) - 32.0;
    let cols = 5usize.min(count);
    // Floored: a fraction over and the row wraps a card early.
    ((room - 12.0 * (cols as f32 - 1.0)) / cols as f32).floor()
}

/// The overlay's night, behind every theme.
const NIGHT: u32 = 0x0b0a14;
/// A card's face.
const FACE: u32 = 0x15131f;
/// The bottom of a drawn pack and a card's back.
const DUSK: u32 = 0x1e1b2e;
/// How wide a banner's tab is, so the highlight can glide between them.
const TAB: f32 = 176.0;

/// A pull being shown: what dropped, and where the reveal is.
pub(super) struct Show {
    pull: pb::GachaPull,
    banner: pb::GachaBanner,
    at: Instant,
    ranks: Vec<u8>,
    plan: RevealPlan,
    /// What to play when: the charge, the burst, each card's flip and chime.
    cues: Vec<(u64, Vec<GachaCue>)>,
    heard: usize,
    skipped: bool,
    /// Sharing it: the channels are showing, it's being sent, where it went.
    share_open: bool,
    sharing: bool,
    shared_in: Option<String>,
    /// What went wrong sharing or saving it.
    note: Option<String>,
}

impl Show {
    pub(super) fn new(pull: pb::GachaPull, banner: pb::GachaBanner, reduced: bool) -> Self {
        let ranks: Vec<u8> = pull.results.iter().map(|r| eco::rarity(&banner, &r.tier_id)).collect();
        let plan = eco::reveal_plan(&ranks, reduced);
        let cues = if reduced {
            vec![(0, vec![GachaCue::rarity(ranks.iter().copied().max().unwrap_or(0))])]
        } else {
            let mut cues = vec![(0, vec![GachaCue::Charge]), (plan.burst, vec![GachaCue::Burst])];
            cues.extend(plan.flips.iter().zip(&ranks).map(|(at, r)| (*at, vec![GachaCue::Flip, GachaCue::rarity(*r)])));
            cues
        };
        Self {
            pull,
            banner,
            at: Instant::now(),
            ranks,
            plan,
            cues,
            heard: 0,
            skipped: false,
            share_open: false,
            sharing: false,
            shared_in: None,
            note: None,
        }
    }

    fn elapsed(&self) -> u64 {
        if self.skipped { u64::MAX / 2 } else { self.at.elapsed().as_millis() as u64 }
    }

    pub(super) fn done(&self) -> bool {
        self.elapsed() >= self.plan.done
    }

    /// Jumps to every card turned, with the best one's chime.
    pub(super) fn skip(&mut self) -> Option<GachaCue> {
        if self.done() {
            return None;
        }
        self.skipped = true;
        self.heard = self.cues.len();
        Some(GachaCue::rarity(self.ranks.iter().copied().max().unwrap_or(0)))
    }

    /// The cues whose time has come since last asked.
    fn due(&mut self) -> Vec<GachaCue> {
        let e = self.elapsed();
        let mut out = Vec::new();
        while let Some((at, cues)) = self.cues.get(self.heard).filter(|(at, _)| *at <= e) {
            let _ = at;
            out.extend(cues.iter().copied());
            self.heard += 1;
        }
        out
    }
}

/// A tier's color by its place among the banner's tiers (past the end: the commonest color).
fn tier_rgb(b: &pb::GachaBanner, at: usize) -> Rgba {
    let color = b.tiers.get(at).map_or(eco::TIER_COLORS[eco::TIER_COLORS.len() - 1], |t| eco::tier_color(t, at));
    gpui_kit::rgb(color)
}

/// The banner's own color: its rarest tier that can drop.
fn glow_of(b: &pb::GachaBanner) -> Rgba {
    tier_rgb(b, b.tiers.iter().position(|t| t.rate > 0).unwrap_or(0))
}

fn rgb(c: u32) -> Rgba {
    gpui_kit::rgb(c)
}

fn white(a: f32) -> Hsla {
    alpha(rgb(0xffffff), a)
}

fn shadow(color: Hsla, y: f32, blur: f32, spread: f32) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow {
        color,
        offset: gpui_kit::point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

/// `t` eased in (slow, then fast).
fn ease_in(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(2)
}

/// `t` eased out with a little overshoot, like the web's springs.
fn ease_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.2 * t.powi(3) + 1.2 * t.powi(2)
}

/// The box or card pack: the server's picture, or a pack drawn in the banner's color.
fn pack(art: &str, glow: Rgba, w: f32, h: f32) -> AnyElement {
    if !art.is_empty() {
        return picture(art.to_owned()).w(px(w)).h(px(h)).object_fit(ObjectFit::Contain).into_any_element();
    }
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .rounded(radius_2xl())
        .border_2()
        .border_color(white(0.4))
        .bg(linear_gradient(160.0, linear_color_stop(glow, 0.0), linear_color_stop(rgb(DUSK), 1.0)))
        .shadow(vec![shadow(alpha(glow, 0.4), 10.0, 28.0, 0.0)])
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0xffffff))
        .child(div().absolute().left_0().right_0().top(px(h * 0.13)).h(px(h * 0.07)).bg(white(0.3)))
        .child(div().absolute().left_0().right_0().bottom(px(h * 0.13)).h(px(h * 0.03)).bg(white(0.2)))
        .child(icon("sparkles").size(px(w * 0.4)))
        .into_any_element()
}

/// Light rays around (`cx`, `cy`) in a box, `count` bars from the middle out
/// `reach` long, fading as they go, turned `turn` of the way round.
fn rays(color: Rgba, cx: f32, cy: f32, reach: f32, count: usize, turn: f32, opacity: f32) -> gpui_kit::Div {
    let width = reach * 2.0 * 0.09;
    let mut layer = div().absolute().inset_0().opacity(opacity);
    for n in 0..count {
        let angle = (n as f32 / count as f32 + turn) * TAU;
        layer = layer.child(
            div()
                .absolute()
                .left(px(cx - width / 2.0))
                .top(px(cy))
                .w(px(width))
                .h(px(reach))
                .transform_origin(0.5, 0.0)
                .rotate(radians(angle))
                .bg(linear_gradient(180.0, linear_color_stop(color, 0.0), linear_color_stop(alpha(color, 0.0), 1.0))),
        );
    }
    layer
}

/// "★★★" in a tier's color.
fn star_row(n: usize, color: Rgba, size: f32) -> gpui_kit::Div {
    div().flex_none().text_size(px(size)).line_height(px(size + 2.0)).text_color(color).child("★".repeat(n))
}

/// Words with an amount drawn inside them where `key`'s `{amount}` goes.
fn with_amount(key: &str, amount: AnyElement) -> gpui_kit::Div {
    let text = t_with(key, &[("amount", Arg::Str("\u{0}"))]);
    let (before, after) = text.split_once('\u{0}').unwrap_or((&text, ""));
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .when(!before.trim().is_empty(), |el| el.child(before.trim().to_owned()))
        .child(amount)
        .when(!after.trim().is_empty(), |el| el.child(after.trim().to_owned()))
}

impl ShopView {
    fn gacha_sounds_on(&self) -> bool {
        let prefs = self.core.prefs_now();
        prefs.gacha_sounds && prefs.sounds_on()
    }

    pub(super) fn cue(&self, cues: &[GachaCue]) {
        if cues.is_empty() || !self.gacha_sounds_on() {
            return;
        }
        let prefs = self.core.prefs_now();
        sounds::play_gacha(cues, prefs.volume, &prefs.output_device);
    }

    /// The banners you can pull on: the one picked on its stage, the others in tabs above.
    pub(super) fn gacha_screen(
        &mut self,
        f: &Facts,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = now_ms();
        let banners: Vec<pb::GachaBanner> = f.economy.shown_banners(now).into_iter().cloned().collect();
        if banners.is_empty() {
            return empty("dices", &t("chattools.economy.gacha.empty"), p);
        }
        let at = self.picked.as_ref().and_then(|id| banners.iter().position(|b| &b.id == id)).unwrap_or(0);
        let b = banners[at].clone();
        let mut col = div().flex().flex_col().gap(px(12.0));
        if banners.len() > 1 {
            let x = motion::follow("shop-gacha-tab-hl", at as f32 * (TAB + 4.0), window, cx);
            let mut tabs = div().relative().flex().gap(px(4.0)).child(
                div()
                    .absolute()
                    .top_0()
                    .left(px(x))
                    .w(px(TAB))
                    .h(px(44.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.background)
                    .shadow(vec![shadow(alpha(rgb(0x000000), 0.06), 1.0, 2.0, 0.0)]),
            );
            for (n, banner) in banners.iter().enumerate() {
                let on = n == at;
                let id = banner.id.clone();
                let hover = alpha(p.muted, 0.7);
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!("shop-gacha-tab-{}", banner.id)))
                        .relative()
                        .w(px(TAB))
                        .h(px(44.0))
                        .flex_none()
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_2xl())
                        .text_sm()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(if on { p.foreground } else { p.muted_foreground })
                        .cursor_pointer()
                        .when(!on, |el| el.hover(move |s| s.bg(hover)))
                        .active(|s| s.scale(0.97))
                        .child(div().size(px(28.0)).flex_none().flex().items_center().justify_center().child(pack(
                            &banner.art_url,
                            glow_of(banner),
                            if banner.art_url.is_empty() { 22.0 } else { 28.0 },
                            28.0,
                        )))
                        .child(div().flex_1().min_w_0().truncate().child(banner.name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.picked.as_deref() != Some(id.as_str()) && !on {
                                this.picked = Some(id.clone());
                                this.cue(&[GachaCue::Tap]);
                                cx.notify();
                            }
                        })),
                );
            }
            col = col.child(tabs);
        }
        col = col.child(motion::rise(
            self.stage(&b, f, p, now, window, cx),
            SharedString::from(format!("shop-gacha-stage-{}", b.id)),
            Duration::ZERO,
            10.0,
        ));
        div().id("shop-gacha-scroll").flex_1().min_h_0().overflow_y_scroll().pb(px(24.0)).child(col).into_any_element()
    }

    /// Why `count` pulls can't be made now, if they can't.
    fn why_not(&self, b: &pb::GachaBanner, f: &Facts, count: i32, now: i64) -> Option<String> {
        let price = eco::pull_cost(b, count)?;
        let left = eco::pulls_left(b, f.economy.progress.get(&b.id));
        if !matches!(eco::openness(b, now), eco::Openness::Open(_)) {
            Some(t("chattools.economy.gacha.notOpen"))
        } else if left.is_some_and(|l| l < count) {
            Some(t("chattools.economy.gacha.noPulls"))
        } else if f.economy.amount(&b.currency_id) < price {
            let name = f.economy.currency(&b.currency_id).map(|c| c.name.clone()).unwrap_or_default();
            Some(t_with("chattools.economy.notEnough", &[("currency", Arg::Str(&name))]))
        } else {
            None
        }
    }

    fn stage(
        &mut self,
        b: &pb::GachaBanner,
        f: &Facts,
        p: &Palette,
        now: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let currency = f.economy.currency(&b.currency_id).cloned().unwrap_or_default();
        let progress = f.economy.progress.get(&b.id);
        let glow = glow_of(b);
        let lead = b.tiers.iter().position(|t| t.rate > 0).unwrap_or(0);
        let open = eco::openness(b, now);
        let when = match open {
            eco::Openness::Soon(at) => {
                t_with("chattools.economy.gacha.opensIn", &[("time", Arg::Str(&countdown(at - now)))])
            }
            eco::Openness::Open(at) if at > 0 => {
                t_with("chattools.economy.gacha.closesIn", &[("time", Arg::Str(&countdown(at - now)))])
            }
            _ => t("chattools.economy.gacha.always"),
        };
        let left = eco::pulls_left(b, progress);
        let pity = eco::pity_in(b, progress);
        let width = f32::from(window.viewport_size().width);
        // The content column's width (the shop's side and paddings off), for where the rays turn.
        let stage_w = (width - (240.0 + ((width - 1072.0).max(0.0) / 2.0)) - 40.0 - 104.0).max(320.0);
        let stage_h = 224.0;

        // The stage: the pack over its rays, the banner's name and when it closes. A turned
        // element is only clipped to its mask's bounding box, so the rays stay short enough
        // to end inside the stage on their own.
        let turn =
            move |el: gpui_kit::Div, t: f32| el.child(rays(glow, stage_w - 112.0, stage_h / 2.0, 104.0, 12, t, 0.45));
        let ray_layer =
            motion::ambient(div().absolute().inset_0(), "shop-gacha-rays", Duration::from_secs(40), window, turn);
        let art_w = if b.art_url.is_empty() { 112.0 } else { 144.0 };
        let art_h = if b.art_url.is_empty() { 144.0 } else { 176.0 };
        let floating = motion::ambient(
            div()
                .absolute()
                .right(px(40.0))
                .top(px((stage_h - art_h) / 2.0))
                .child(pack(&b.art_url, glow, art_w, art_h)),
            "shop-gacha-pack-bob",
            Duration::from_millis(3200),
            window,
            |el, t| {
                let s = (t * TAU).sin();
                el.translate_y(px(-4.0 + 4.0 * (t * TAU).cos() - 4.0 * 0.0)).rotate(radians((2.0 * s).to_radians()))
            },
        );
        let sounds_on = self.gacha_sounds_on();
        let top = b.tiers.get(lead).cloned();
        let mut words = div().relative().max_w(relative(0.6)).p(px(20.0)).flex().flex_col().items_start().gap(px(8.0));
        if let Some(tier) = &top {
            words = words.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(20.0))
                    .rounded_full()
                    .bg(glow)
                    .text_color(rgb(0xffffff))
                    .text_size(px(11.2))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .shadow(vec![shadow(alpha(rgb(0x000000), 0.08), 1.0, 2.0, 0.0)])
                    .child("★")
                    .child(format!("{} · {}", tier.name.to_uppercase(), eco::percent(tier.rate))),
            );
        }
        words = words
            .child(
                div()
                    .text_size(px(30.0))
                    .line_height(px(34.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(crate::ui::text::tracked(b.name.clone(), crate::ui::text::TIGHT).wraps()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(20.0))
                    .rounded_full()
                    .bg(alpha(p.background, 0.6))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(icon("clock").size(px(12.0)))
                    .child(when),
            )
            .when(!b.description.is_empty(), |el| {
                el.child(div().text_sm().text_color(p.muted_foreground).line_clamp(3).child(b.description.clone()))
            });
        let hero = div()
            .relative()
            .min_h(px(stage_h))
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .child(div().absolute().inset_0().bg(linear_gradient(
                120.0,
                linear_color_stop(alpha(glow, 0.22), 0.0),
                linear_color_stop(alpha(glow, 0.0), 0.65),
            )))
            // The glow the pack floats in.
            .child(
                div()
                    .absolute()
                    .left(px(stage_w - 112.0 - 60.0))
                    .top(px(stage_h / 2.0 - 60.0))
                    .size(px(120.0))
                    .rounded_full()
                    .shadow(vec![shadow(alpha(glow, 0.3), 0.0, 120.0, 60.0)]),
            )
            .child(ray_layer)
            .child(floating)
            .child(
                div()
                    .id("shop-gacha-sounds")
                    .absolute()
                    .top(px(12.0))
                    .right(px(12.0))
                    .size(px(32.0))
                    .rounded_full()
                    .bg(alpha(rgb(0x000000), 0.25))
                    .text_color(rgb(0xffffff))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(rgb(0x000000), 0.4)))
                    .active(|s| s.scale(0.9))
                    .child(motion::pop(
                        icon(if sounds_on { "volume-2" } else { "volume-x" }).size(px(16.0)),
                        SharedString::from(format!("shop-gacha-sounds-{sounds_on}")),
                        0.4,
                        0.0,
                        Duration::ZERO,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sounds(cx))),
            )
            .child(words);

        let mut col = div().flex().flex_col().gap(px(12.0)).child(hero);

        // What the rarest tier holds.
        let prizes: Vec<&pb::GachaPrize> = top
            .as_ref()
            .map(|tier| b.prizes.iter().filter(|x| x.tier_id == tier.id).take(4).collect())
            .unwrap_or_default();
        if !prizes.is_empty() {
            let mut row = div().flex().flex_wrap().items_center().gap(px(6.0)).child(
                div()
                    .mr(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(t("chattools.economy.gacha.topPrizes")),
            );
            for (n, prize) in prizes.into_iter().enumerate() {
                row = row.child(motion::pop(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(26.0))
                        .px(px(10.0))
                        .rounded_full()
                        .border_1()
                        .border_color(alpha(glow, 0.4))
                        .bg(alpha(glow, 0.08))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(icon("sparkles").size(px(12.0)).text_color(glow))
                        .child(
                            div()
                                .max_w(px(160.0))
                                .truncate()
                                .child(eco::prize_name(&f.economy, prize, &f.roles, &f.items)),
                        )
                        .child(div().text_color(p.muted_foreground).child(eco::percent(prize.chance))),
                    SharedString::from(format!("shop-gacha-top-{}", prize.id)),
                    0.8,
                    0.0,
                    Duration::from_millis(80 + 50 * n as u64),
                ));
            }
            col = col.child(row);
        }

        // Pity, as a meter filling toward the guarantee.
        if let (Some(n), Some(at)) = (pity, b.tiers.iter().position(|t| t.id == b.pity_tier_id))
            && b.pity_pulls > 0
        {
            let tint = tier_rgb(b, at);
            let done = b.pity_pulls - n;
            let frac = (done as f32 / b.pity_pulls as f32).max(0.03);
            let fill = motion::follow(SharedString::from(format!("shop-gacha-pity-{}", b.id)), frac, window, cx);
            let sheen = motion::ambient(
                div().absolute().top_0().bottom_0().w(px(40.0)).bg(linear_gradient(
                    90.0,
                    linear_color_stop(white(0.0), 0.0),
                    linear_color_stop(white(0.6), 0.5),
                )),
                "shop-gacha-pity-sheen",
                Duration::from_millis(3000),
                window,
                |el, t| el.left(relative((t / 0.6).min(1.0) * 1.5 - 0.4)),
            );
            // How pity works, on hover, as the web's title.
            let about = t_with(
                "chattools.economy.gacha.pityAbout",
                &[("tier", Arg::Str(&b.tiers[at].name)), ("count", Arg::Num(i64::from(b.pity_pulls)))],
            );
            col = col.child(
                div()
                    .id(SharedString::from(format!("shop-gacha-pity-box-{}", b.id)))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(about.clone()).build(window, cx))
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(icon("shield-check").size(px(16.0)).text_color(tint))
                            .child(div().flex_1().min_w_0().child(t_with(
                                "chattools.economy.gacha.pityIn",
                                &[("count", Arg::Num(i64::from(n))), ("tier", Arg::Str(&b.tiers[at].name))],
                            )))
                            .child(t_with(
                                "chattools.economy.gacha.pityMeter",
                                &[("done", Arg::Num(i64::from(done))), ("total", Arg::Num(i64::from(b.pity_pulls)))],
                            )),
                    )
                    .child(
                        div().h(px(8.0)).rounded_full().overflow_hidden().bg(p.muted).child(
                            div()
                                .relative()
                                .h_full()
                                .w(relative(fill.clamp(0.0, 1.0)))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(tint)
                                .child(sheen),
                        ),
                    ),
            );
        }

        // Pull once, and the multi-pull.
        let busy = self.pulling;
        let mut pulls = div().flex().gap(px(8.0));
        for count in [1, b.multi_pull].into_iter().filter(|c| *c == 1 || *c >= 2) {
            let Some(price) = eco::pull_cost(b, count) else { continue };
            let no = self.why_not(b, f, count, now);
            let ok = no.is_none() && busy.is_none();
            let multi = count > 1;
            let label = if multi {
                t_with("chattools.economy.gacha.pullMany", &[("count", Arg::Num(i64::from(count)))])
            } else {
                t("chattools.economy.gacha.pullOnce")
            };
            let banner = b.clone();
            let id = SharedString::from(format!("shop-pull-{}-{count}", b.id));
            let mut button = div()
                .id(id.clone())
                .group(id.clone())
                .relative()
                .flex_1()
                .h(px(64.0))
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .font_weight(FontWeight::EXTRA_BOLD);
            button = if multi {
                button
                    .border_color(alpha(glow, 0.0))
                    .text_color(rgb(0xffffff))
                    .bg(linear_gradient(135.0, linear_color_stop(glow, 0.0), linear_color_stop(p.primary, 1.0)))
                    .shadow(vec![shadow(alpha(glow, 0.33), 8.0, 24.0, 0.0)])
            } else {
                let hover = alpha(p.muted, 0.6);
                button
                    .border_color(p.border)
                    .bg(p.background)
                    .shadow(vec![shadow(alpha(rgb(0x000000), 0.05), 1.0, 2.0, 0.0)])
                    .when(ok, |el| el.hover(move |s| s.bg(hover)))
            };
            if multi && no.is_none() {
                button = button.child(motion::ambient(
                    div().absolute().top_0().bottom_0().w(px(64.0)).bg(linear_gradient(
                        90.0,
                        linear_color_stop(white(0.0), 0.0),
                        linear_color_stop(white(0.25), 0.5),
                    )),
                    SharedString::from(format!("shop-pull-sheen-{}", b.id)),
                    Duration::from_millis(4000),
                    window,
                    |el, t| el.left(relative((t / 0.4).min(1.0) * 1.6 - 0.3)),
                ));
            }
            let glyph = if busy == Some(count) {
                spinner(SharedString::from(format!("shop-pull-spin-{count}")), 16.0, window)
            } else {
                div()
                    .id(SharedString::from(format!("shop-pull-dice-{count}")))
                    .group_hover(id.clone(), |s| s.rotate(radians(PI / 4.0)))
                    .child(icon("dices").size(px(16.0)))
                    .into_any_element()
            };
            button = button
                .child(div().relative().flex().items_center().gap(px(6.0)).text_base().child(glyph).child(label))
                .child(div().relative().text_xs().opacity(0.85).child(money(&currency, &f.emojis, price, 12.0)))
                .when(!ok, |el| el.opacity(0.5))
                .when(ok, |el| {
                    el.cursor_pointer()
                        .active(|s| s.scale(0.97))
                        .on_click(cx.listener(move |this, _, _, cx| this.pull(&banner, count, cx)))
                });
            pulls = pulls.child(button);
        }
        col = col.child(pulls);

        // What you have, your limits, and the rates.
        let rates_open = self.rates_open;
        let mut meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(16.0))
            .gap_y(px(4.0))
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(with_amount(
                "chattools.economy.gacha.youHave",
                div()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.foreground)
                    .child(money(&currency, &f.emojis, f.economy.amount(&b.currency_id), 12.0))
                    .into_any_element(),
            ));
        if let Some(n) = left {
            meta = meta.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("ticket").size(px(14.0)))
                    .child(t_with("chattools.economy.gacha.pullsLeft", &[("count", Arg::Num(i64::from(n)))])),
            );
        }
        if let Some(pr) = progress.filter(|pr| pr.pulls > 0) {
            meta = meta.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("dices").size(px(14.0)))
                    .child(t_with("chattools.economy.gacha.pulled", &[("count", Arg::Num(i64::from(pr.pulls)))])),
            );
        }
        let (hover_bg, hover_fg) = (p.muted, p.foreground);
        meta = meta.child(
            div()
                .id("shop-gacha-rates")
                .ml_auto()
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(radius_lg())
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .child(icon("percent").size(px(14.0)))
                .child(t("chattools.economy.gacha.rates"))
                .child(
                    div()
                        .id("shop-gacha-rates-chevron")
                        .when(rates_open, |el| el.rotate(radians(PI)))
                        .child(icon("chevron-down").size(px(14.0))),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.rates_open = !this.rates_open;
                    cx.notify();
                })),
        );
        col = col.child(meta);
        let blocked = [1, b.multi_pull].into_iter().find_map(|c| self.why_not(b, f, c, now));
        if let Some(why) = blocked.filter(|_| busy.is_none() && self.error.is_none()) {
            col = col.child(div().text_xs().text_color(p.muted_foreground).child(why));
        }
        if rates_open {
            col = col.child(motion::rise(self.rates(b, f, p), "shop-gacha-rates-in", Duration::ZERO, -6.0));
        }
        col
    }

    fn toggle_sounds(&mut self, cx: &mut Context<Self>) {
        let on = !self.core.prefs_now().gacha_sounds;
        self.core.set_prefs(|p| p.gacha_sounds = on);
        if on {
            self.cue(&[GachaCue::Tap]);
        }
        cx.notify();
    }

    /// Every tier with its rate (a bar of them all first), and each prize in it with its chance per pull.
    fn rates(&self, b: &pb::GachaBanner, f: &Facts, p: &Palette) -> gpui_kit::Div {
        let mut bar = div().h(px(10.0)).flex().gap(px(2.0)).overflow_hidden().rounded_full();
        let total: i32 = b.tiers.iter().map(|t| t.rate).sum::<i32>().max(1);
        for (n, tier) in b.tiers.iter().enumerate() {
            bar = bar.child(
                div()
                    .h_full()
                    .min_w(px(4.0))
                    .flex_shrink(1.0)
                    .w(relative(tier.rate as f32 / total as f32))
                    .rounded_full()
                    .bg(tier_rgb(b, n)),
            );
        }
        let mut list = div()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .child(bar);
        for (n, tier) in b.tiers.iter().enumerate() {
            let tint = tier_rgb(b, n);
            let mut block = div().flex().flex_col().gap(px(4.0)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_sm()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(div().size(px(10.0)).rounded_full().bg(tint))
                    .child(div().text_color(tint).child(tier.name.clone()))
                    .child(star_row(eco::stars(b, &tier.id), tint, 10.0))
                    .child(div().ml_auto().child(eco::percent(tier.rate))),
            );
            for prize in b.prizes.iter().filter(|x| x.tier_id == tier.id) {
                block = block.child(
                    div()
                        .pl(px(18.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(eco::prize_name(&f.economy, prize, &f.roles, &f.items)),
                        )
                        .child(eco::percent(prize.chance)),
                );
            }
            list = list.child(block);
        }
        list.child(
            div().text_size(px(11.2)).text_color(p.muted_foreground).child(t("chattools.economy.gacha.ratesNote")),
        )
    }

    /// A pull playing out over the whole window, then what dropped, with Pull again and Done.
    pub(super) fn pull_show(
        &mut self,
        f: &Facts,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let due = self.show.as_mut()?.due();
        self.cue(&due);
        let mut arts = self.won_art(f, f32::from(window.viewport_size().width), p, cx);
        let show = self.show.as_ref()?;
        let reduced = cx.reduce_motion();
        let e = show.elapsed();
        let done = show.done();
        let plan = &show.plan;
        let b = &show.banner;
        let best = eco::best_tier(b, &show.pull.results);
        let color = tier_rgb(b, best);
        let count = show.pull.results.len();
        let size = window.viewport_size();
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let stage = if e < plan.burst {
            0
        } else if e < plan.deal {
            1
        } else {
            2
        };
        // The reveal plays out even behind another window (it's short, and its sounds keep
        // time with it); only the slow rays after it wait for the window to be looked at.
        if !reduced && (!done || window.is_window_active()) {
            window.request_animation_frame();
        }
        let secs = show.at.elapsed().as_secs_f32();
        // The rays behind it all: quick while it charges, slow once the cards are out.
        let turn = if reduced {
            0.0
        } else if stage == 2 {
            secs / 60.0
        } else {
            secs / 8.0
        };
        let mut night = div()
            .id("shop-gacha-show")
            .absolute()
            .inset_0()
            .occlude()
            .overflow_hidden()
            .bg(rgb(NIGHT))
            .text_color(rgb(0xffffff))
            .flex()
            .flex_col()
            .items_center()
            .px(px(16.0))
            .py(px(24.0))
            // The best tier's light, spread from the middle.
            .child(
                div()
                    .absolute()
                    .left(px(w / 2.0 - 120.0))
                    .top(px(h * 0.45 - 120.0))
                    .size(px(240.0))
                    .rounded_full()
                    .shadow(vec![shadow(alpha(color, 0.33), 0.0, 360.0, 220.0)]),
            )
            .child(rays(color, w / 2.0, h / 2.0, 480.0, 16, turn, if stage == 2 { 0.18 } else { 0.3 }));

        // The banner's name, what's happening, the sounds and Skip.
        let sounds_on = self.gacha_sounds_on();
        let about = if stage == 2 {
            t_with("chattools.economy.gacha.revealAbout", &[("count", Arg::Num(count as i64))])
        } else {
            t_with("chattools.economy.gacha.opening", &[("banner", Arg::Str(&b.name))])
        };
        let header = div()
            .relative()
            .w_full()
            .max_w(px(896.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).truncate().child(b.name.clone()))
                    .child(div().text_sm().text_color(white(0.7)).child(about)),
            )
            .child(
                div()
                    .id("shop-gacha-show-sounds")
                    .size(px(32.0))
                    .rounded_full()
                    .bg(alpha(rgb(0x000000), 0.25))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(rgb(0x000000), 0.4)))
                    .active(|s| s.scale(0.9))
                    .child(icon(if sounds_on { "volume-2" } else { "volume-x" }).size(px(16.0)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sounds(cx))),
            )
            .when(!done, |el| {
                el.child(
                    div()
                        .id("shop-gacha-skip")
                        .h(px(32.0))
                        .px(px(14.0))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(white(0.1))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.bg(white(0.2)))
                        .active(|s| s.scale(0.95))
                        .child(t("chattools.economy.gacha.skip"))
                        .on_click(cx.listener(|this, _, _, cx| this.skip_show(cx))),
                )
            });
        night = night.child(header);

        // The middle: the pack charging and bursting, then the cards.
        let mut middle = div()
            .id("shop-gacha-middle")
            .relative()
            .w_full()
            .max_w(px(896.0))
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .py(px(24.0));
        if stage < 2 {
            let k = (e as f32 / reveal::CHARGE as f32).min(1.0);
            let (pw, ph) = if b.art_url.is_empty() { (176.0, 224.0) } else { (208.0, 256.0) };
            let (scale, fade) = if stage == 1 {
                let k = ((e - plan.burst) as f32 / reveal::BURST as f32).min(1.0);
                (1.06 + 0.74 * ease_in(k), 1.0 - ease_in(k))
            } else {
                (0.8 + 0.26 * ease_in(k), 1.0)
            };
            // A shake that grows as it charges.
            let wobble = if stage == 0 { (k * 9.0 * PI).sin() * 10.0 * k } else { 0.0 };
            let orbit = ease_in(k) * 2.0 * TAU;
            let spark = |r: f32, size: f32, a: f32| {
                div()
                    .absolute()
                    .left(px(144.0 + r * a.sin() - size / 2.0))
                    .top(px(144.0 - r * a.cos() - size / 2.0))
                    .size(px(size))
                    .rounded_full()
                    .bg(rgb(0xffffff))
                    .shadow(vec![shadow(color.into(), 0.0, 18.0, 6.0)])
            };
            middle = middle.on_click(cx.listener(|this, _, _, cx| this.skip_show(cx))).child(
                div()
                    .relative()
                    .size(px(288.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(fade)
                    .child(
                        div()
                            .absolute()
                            .left(px(144.0 - 80.0))
                            .top(px(144.0 - 80.0))
                            .size(px(160.0))
                            .rounded_full()
                            .shadow(vec![shadow(alpha(color, 0.15 + 0.4 * ease_in(k)), 0.0, 120.0, 40.0 + 60.0 * k)]),
                    )
                    .child(spark(144.0, 12.0, orbit))
                    .child(spark(144.0, 8.0, orbit + PI))
                    .child(
                        div().scale(scale).rotate(radians(wobble.to_radians())).child(pack(&b.art_url, color, pw, ph)),
                    ),
            );
        } else {
            let single = count == 1;
            let room = w.min(896.0 + 32.0) - 32.0;
            let card_w = card_width(count, w);
            let mut grid =
                div().flex().flex_wrap().justify_center().gap(px(12.0)).w(px(if single { card_w } else { room }));
            for (n, art) in arts.iter_mut().enumerate() {
                grid = grid.child(self.card(show, n, e, card_w, art.take(), f, p));
            }
            middle = middle.child(grid);
        }
        night = night.child(middle);

        // Pull again and Done, once every card has turned.
        if done {
            let count_i = count as i32;
            let price = eco::pull_cost(b, count_i);
            let currency = f.economy.currency(&b.currency_id).cloned().unwrap_or_default();
            let can = self.why_not(b, f, count_i, now_ms()).is_none();
            let banner = b.clone();
            let busy = self.pulling.is_some();
            let mut footer = div().relative().flex().flex_wrap().justify_center().gap(px(8.0));
            if let (Some(price), true) = (price, can) {
                footer = footer.child(
                    div()
                        .id("shop-gacha-again")
                        .h(px(48.0))
                        .px(px(20.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_2xl())
                        .bg(white(0.1))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .cursor_pointer()
                        .hover(|s| s.bg(white(0.2)))
                        .active(|s| s.scale(0.95))
                        .child(if busy {
                            spinner("shop-gacha-again-spin", 16.0, window)
                        } else {
                            icon("rotate-ccw").size(px(16.0)).into_any_element()
                        })
                        .child(t("chattools.economy.gacha.again"))
                        .child(div().text_sm().text_color(white(0.8)).child(money(&currency, &f.emojis, price, 14.0)))
                        .when(!busy, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| this.pull(&banner, count_i, cx)))
                        }),
                );
            }
            if self.core.share_here(&self.key) {
                let icon_name = if show.shared_in.is_some() { "check" } else { "share-2" };
                footer = footer.child(
                    div()
                        .id("shop-gacha-share")
                        .h(px(48.0))
                        .px(px(20.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_2xl())
                        .bg(white(if show.share_open { 0.2 } else { 0.1 }))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .cursor_pointer()
                        .hover(|s| s.bg(white(0.2)))
                        .active(|s| s.scale(0.95))
                        .child(if show.sharing {
                            spinner("shop-gacha-share-spin", 16.0, window)
                        } else {
                            icon(icon_name).size(px(16.0)).into_any_element()
                        })
                        .child(t("chattools.economy.gacha.share"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(show) = this.show.as_mut() {
                                show.share_open = !show.share_open;
                                show.note = None;
                            }
                            cx.notify();
                        })),
                );
            }
            footer = footer.child(
                div()
                    .id("shop-gacha-save")
                    .h(px(48.0))
                    .px(px(20.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_2xl())
                    .bg(white(0.1))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .cursor_pointer()
                    .hover(|s| s.bg(white(0.2)))
                    .active(|s| s.scale(0.95))
                    .child(icon("download").size(px(16.0)))
                    .child(t("chattools.economy.gacha.saveImage"))
                    .on_click(cx.listener(|this, _, _, cx| this.save_image(cx))),
            );
            footer = footer.child(
                div()
                    .id("shop-gacha-done")
                    .h(px(48.0))
                    .px(px(28.0))
                    .flex()
                    .items_center()
                    .rounded(radius_2xl())
                    .bg(linear_gradient(135.0, linear_color_stop(color, 0.0), linear_color_stop(p.primary, 1.0)))
                    .shadow(vec![shadow(alpha(rgb(0x000000), 0.25), 10.0, 15.0, -3.0)])
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.92))
                    .active(|s| s.scale(0.95))
                    .child(t("chattools.economy.gacha.done"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show = None;
                        cx.notify();
                    })),
            );
            let mut end = div().relative().flex().flex_col().items_center().gap(px(10.0));
            if show.share_open {
                end = end.child(self.share_picker(show, f, cx));
            }
            if let Some(note) = &show.note {
                end = end.child(div().text_sm().text_color(white(0.8)).child(note.clone()));
            }
            night = night.child(motion::rise(
                end.child(footer),
                SharedString::from(format!("shop-gacha-end-{}", show.pull.id)),
                Duration::ZERO,
                16.0,
            ));
        }

        // A white flash as it bursts.
        if stage == 1 && !reduced {
            let k = (e - plan.burst) as f32 / 450.0;
            let flash = if k < 0.25 { k / 0.25 * 0.85 } else { (0.85 * (1.0 - (k - 0.25) / 0.75)).max(0.0) };
            night = night.child(div().absolute().inset_0().bg(white(flash)));
        }
        Some(
            motion::fade_in(
                night,
                SharedString::from(format!("shop-gacha-show-{}", show.pull.id)),
                Duration::from_millis(200),
            )
            .into_any_element(),
        )
    }

    /// The channels a pull can be shared in, as chips over the buttons.
    fn share_picker(&self, show: &Show, f: &Facts, cx: &mut Context<Self>) -> AnyElement {
        let mut chips = div().flex().flex_wrap().justify_center().gap(px(6.0)).max_w(px(560.0));
        if f.channels.is_empty() {
            chips = chips.child(div().text_sm().text_color(white(0.7)).child(t("chattools.economy.gacha.noChannels")));
        }
        for (id, name) in &f.channels {
            let (id, name) = (id.clone(), name.clone());
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("shop-gacha-share-in|{id}")))
                    .h(px(32.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded_full()
                    .bg(white(0.12))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s| s.bg(white(0.24)))
                    .active(|s| s.scale(0.95))
                    .child(icon("hash").size(px(14.0)).text_color(white(0.6)))
                    .child(name.clone())
                    .when(!show.sharing, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| this.share_in(id.clone(), name.clone(), cx)))
                    }),
            );
        }
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(white(0.6))
                    .child(t("chattools.economy.gacha.shareIn")),
            )
            .child(chips)
            .into_any_element()
    }

    pub(super) fn share_in(&mut self, channel_id: String, name: String, cx: &mut Context<Self>) {
        let Some(show) = self.show.as_mut().filter(|s| !s.sharing) else { return };
        show.sharing = true;
        show.note = None;
        let pull_id = show.pull.id.clone();
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let shared = pull_id.clone();
        self.run(
            cx,
            async move { core.share_pull(&key, &sid, &channel_id, &shared).await },
            move |this, result, cx| {
                if let Some(show) = this.show.as_mut().filter(|s| s.pull.id == pull_id) {
                    show.sharing = false;
                    match result {
                        Ok(()) => {
                            show.share_open = false;
                            show.note =
                                Some(t_with("chattools.economy.gacha.sharedIn", &[("channel", Arg::Str(&name))]));
                            show.shared_in = Some(name);
                        }
                        Err(err) => show.note = Some(err.message),
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn save_image(&mut self, cx: &mut Context<Self>) {
        let Some(show) = &self.show else { return };
        let cards = crate::core::gacha_share::cards_of_pull(&show.pull, &show.banner);
        let pull_id = show.pull.id.clone();
        let task = crate::ui::gacha_share::save_image(&cards, &self.facts().my_name, cx);
        cx.spawn(async move |this, cx| {
            if let Some(why) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(show) = this.show.as_mut().filter(|s| s.pull.id == pull_id) {
                        show.note = Some(why);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn skip_show(&mut self, cx: &mut Context<Self>) {
        if let Some(cue) = self.show.as_mut().and_then(|s| s.skip()) {
            self.cue(&[cue]);
        }
        cx.notify();
    }

    /// What each card of the pull showing won, when it's a profile item: worn by you, an
    /// effect playing once its card has turned (`item_art`).
    fn won_art(&mut self, f: &Facts, w: f32, p: &Palette, cx: &mut Context<Self>) -> Vec<Option<AnyElement>> {
        let Some(show) = self.show.as_ref() else { return Vec::new() };
        let e = show.elapsed();
        let count = show.pull.results.len();
        let won: Vec<(Option<pb::ProfileItem>, bool)> = show
            .pull
            .results
            .iter()
            .zip(&show.plan.flips)
            .map(|(r, &flip)| {
                let item = f
                    .economy
                    .listing(&r.listing_id)
                    .filter(|l| l.kind == Kind::ProfileItem as i32)
                    .and_then(|l| f.items.iter().find(|i| i.id == l.target_id))
                    .cloned();
                (item, e >= flip)
            })
            .collect();
        let width = card_width(count, w) * 0.42;
        won.iter()
            .enumerate()
            .map(|(n, (item, turned))| self.item_art(item.as_ref(), f, &format!("gacha|{n}"), width, *turned, p, cx))
            .collect()
    }

    /// One card of the pull: dealt face down, then turned to show its tier and prize.
    #[allow(clippy::too_many_arguments)]
    fn card(
        &self,
        show: &Show,
        n: usize,
        e: u64,
        w: f32,
        art: Option<AnyElement>,
        f: &Facts,
        p: &Palette,
    ) -> AnyElement {
        let _ = p;
        let b = &show.banner;
        let r = &show.pull.results[n];
        let rank = show.ranks[n];
        let plan = &show.plan;
        let h = w * 4.0 / 3.0;
        let tier_at = b.tiers.iter().position(|t| t.id == r.tier_id).unwrap_or(b.tiers.len());
        let color = tier_rgb(b, tier_at);
        let glow = glow_of(b);
        let ms = |a: u64| e.saturating_sub(a) as f32;
        // Dealt in, one after another.
        let dealt = ease_back(ms(plan.deal + n as u64 * reveal::DEAL) / 420.0);
        let flip_at = plan.flips[n];
        let flip = 110.0;
        let since = e as f32 - flip_at as f32;
        let (back_x, face_x) = if since < 0.0 {
            (1.0, 0.0)
        } else if since < flip {
            (1.0 - since / flip, 0.0)
        } else {
            (0.0, ((since - flip) / flip).min(1.0))
        };
        let turned = since >= 0.0;
        // A rare one trembles in its color before it turns.
        let waiting = rank >= 2 && !turned && e + reveal::SUSPENSE >= flip_at && e >= plan.deal;
        let tremble = if waiting { (e as f32 / 400.0 * TAU * 2.0).sin() * 3.0 } else { 0.0 };
        let pop = if rank >= 2 && since > flip && since < flip + 450.0 {
            1.0 + 0.1 * ((since - flip) / 450.0 * PI).sin()
        } else {
            1.0
        };
        let mut slot = div()
            .relative()
            .w(px(w))
            .h(px(h))
            .flex_none()
            .opacity(dealt.clamp(0.0, 1.0))
            .translate_y(px((1.0 - dealt) * 60.0))
            .scale(0.6 + 0.4 * dealt)
            .rotate(radians(((1.0 - dealt) * if n % 2 == 1 { 8.0 } else { -8.0 }).to_radians()));
        if turned && rank >= 3 {
            let reach = w * 1.3;
            slot =
                slot.child(
                    div()
                        .absolute()
                        .left(px(w / 2.0 - reach))
                        .top(px(h / 2.0 - reach))
                        .size(px(reach * 2.0))
                        .child(rays(color, reach, reach, reach, 12, e as f32 / 10_000.0, 0.5)),
                );
        }
        // The back, until it turns.
        let back = div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_2()
            .border_color(white(0.25))
            .bg(linear_gradient(160.0, linear_color_stop(glow, 0.0), linear_color_stop(rgb(0x2a2540), 1.0)))
            .shadow(vec![if waiting {
                shadow(color.into(), 0.0, 28.0, 0.0)
            } else {
                shadow(alpha(rgb(0x000000), 0.25), 10.0, 15.0, -3.0)
            }])
            .flex()
            .items_center()
            .justify_center()
            .scale_x(back_x)
            .rotate(radians(tremble.to_radians()))
            .child(div().absolute().inset(px(8.0)).rounded(radius_xl()).border_1().border_color(white(0.2)))
            .child(if b.art_url.is_empty() {
                icon("sparkles").size(px(w / 3.0)).text_color(white(0.8)).into_any_element()
            } else {
                picture(b.art_url.clone())
                    .size(px(w / 2.0))
                    .object_fit(ObjectFit::Contain)
                    .opacity(0.8)
                    .into_any_element()
            });
        if back_x > 0.0 {
            slot = slot.child(back);
        }
        if face_x > 0.0 {
            slot = slot.child(self.face(show, n, w, color, rank, face_x * pop, pop, e, art, f));
        }
        // Sparks flying out as a rare one turns.
        if rank >= 2 && since > flip && since < flip + 800.0 {
            let k = (since - flip) / 800.0;
            let out = 1.0 - (1.0 - k).powi(3);
            for s in 0..10 {
                let a = s as f32 / 10.0 * TAU;
                let d = 60.0 + (s % 3) as f32 * 18.0;
                let fade = if k < 0.5 { k * 2.0 } else { 2.0 - k * 2.0 };
                slot = slot.child(
                    div()
                        .absolute()
                        .left(px(w / 2.0 + a.cos() * d * out - 6.0))
                        .top(px(h / 2.0 + a.sin() * d * out - 8.0))
                        .text_sm()
                        .text_color(color)
                        .opacity(fade)
                        .child("✦"),
                );
            }
        }
        slot.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn face(
        &self,
        show: &Show,
        n: usize,
        w: f32,
        color: Rgba,
        rank: u8,
        scale_x: f32,
        pop: f32,
        e: u64,
        art: Option<AnyElement>,
        f: &Facts,
    ) -> AnyElement {
        let b = &show.banner;
        let r = &show.pull.results[n];
        let currency = f.economy.currency(&r.currency_id).cloned();
        let listing = f.economy.listing(&r.listing_id);
        let kind = listing.and_then(|l| Kind::try_from(l.kind).ok());
        let glyph = if r.listing_id.is_empty() {
            currency.as_ref().map_or_else(
                || icon("package").size(px(36.0)).into_any_element(),
                |c| currency_icon(c, &f.emojis, 40.0),
            )
        } else {
            match kind {
                Some(Kind::Role) => {
                    let role = listing.and_then(|l| f.roles.iter().find(|x| x.id == l.target_id));
                    let tint: Rgba = role.and_then(|x| x.color).map_or(color, |c| rgb(c as u32));
                    icon("shield").size(px(36.0)).text_color(tint).into_any_element()
                }
                Some(Kind::ProfileItem) => icon("sparkles").size(px(36.0)).into_any_element(),
                _ => icon("package").size(px(36.0)).into_any_element(),
            }
        };
        let name = if r.prize_name.is_empty() {
            listing.map(|l| eco::listing_name(l, &f.roles, &f.items)).unwrap_or_default()
        } else {
            r.prize_name.clone()
        };
        let outcome = pb::GachaOutcome::try_from(r.outcome).unwrap_or(pb::GachaOutcome::Won);
        let note: Option<AnyElement> = match outcome {
            pb::GachaOutcome::Won if r.guaranteed => {
                Some(div().child(t("chattools.economy.gacha.guaranteed")).into_any_element())
            }
            pb::GachaOutcome::Won => None,
            other => {
                let label = if other == pb::GachaOutcome::Duplicate {
                    t("chattools.economy.gacha.duplicate")
                } else {
                    t("chattools.economy.gacha.refunded")
                };
                let refund = currency.as_ref().filter(|_| r.amount > 0).map(|c| money(c, &f.emojis, r.amount, 10.0));
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(label)
                        .when_some(refund, |el, m| el.child(m))
                        .into_any_element(),
                )
            }
        };
        let mut face = div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_2()
            .border_color(color)
            .bg(rgb(FACE))
            .when(rank >= 1, |el| {
                el.shadow(vec![shadow(
                    alpha(color, if rank >= 2 { 0.66 } else { 0.4 }),
                    0.0,
                    8.0 + rank as f32 * 10.0,
                    0.0,
                )])
            })
            .p(px(8.0))
            .flex()
            .flex_col()
            .items_center()
            .scale_x(scale_x)
            .scale_y(pop)
            .child(div().absolute().inset_0().bg(linear_gradient(
                180.0,
                linear_color_stop(alpha(color, 0.3), 0.0),
                linear_color_stop(alpha(color, 0.0), 0.7),
            )));
        if rank >= 3 {
            let k = (e % 2200) as f32 / 1400.0;
            if k <= 1.0 {
                face =
                    face.child(div().absolute().top_0().bottom_0().left(relative(k * 1.9 - 0.6)).w(relative(0.5)).bg(
                        linear_gradient(90.0, linear_color_stop(white(0.0), 0.0), linear_color_stop(white(0.3), 0.5)),
                    ));
            }
        }
        face = face
            .child(div().relative().child(star_row(eco::stars(b, &r.tier_id), color, 12.0)))
            .child(match art {
                Some(art) => div().relative().my_auto().flex_none().child(art),
                None => div()
                    .relative()
                    .my_auto()
                    .size(px(56.0))
                    .flex_none()
                    .rounded_full()
                    .bg(alpha(color, 0.18))
                    .text_color(color)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(glyph),
            })
            .child(
                div()
                    .relative()
                    .w_full()
                    .text_center()
                    .text_size(px(if w > 140.0 { 14.0 } else { 12.0 }))
                    .line_height(px(if w > 140.0 { 17.0 } else { 15.0 }))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .line_clamp(2)
                    .child(name),
            )
            .child(
                div()
                    .relative()
                    .mt(px(2.0))
                    .text_size(px(9.6))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(color)
                    .child(r.tier_name.to_uppercase()),
            )
            .when_some(note, |el, note| {
                el.child(
                    div()
                        .relative()
                        .mt(px(4.0))
                        .px(px(6.0))
                        .rounded_full()
                        .bg(white(0.1))
                        .text_size(px(9.6))
                        .font_weight(FontWeight::BOLD)
                        .text_color(white(0.8))
                        .child(note),
                )
            });
        face.into_any_element()
    }
}
