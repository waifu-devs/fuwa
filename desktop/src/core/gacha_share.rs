//! A gacha pull to show off (docs/economy.md): the cards a message shares
//! (`Message.gacha`) and the picture saved of them. The picture is laid out
//! as the web lays it out (`web/src/lib/gacha-share.ts`), tested against the
//! same file, and drawn here on the CPU with the app's own font.

use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

use crate::core::economy::{self as eco, TIER_COLORS};
use crate::pb;

/// One card, as it's drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ShareCard {
    pub tier_name: String,
    pub prize_name: String,
    pub rarity: u8,
    pub stars: usize,
    /// 0xRRGGBB.
    pub color: u32,
    pub outcome: pb::GachaOutcome,
    pub guaranteed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShareCards {
    pub banner_name: String,
    pub cards: Vec<ShareCard>,
}

fn commonest() -> u32 {
    TIER_COLORS[TIER_COLORS.len() - 1]
}

/// What a shared message carries, with each tier's color settled.
pub fn cards_of_share(share: &pb::GachaShare) -> ShareCards {
    ShareCards {
        banner_name: share.banner_name.clone(),
        cards: share
            .cards
            .iter()
            .map(|c| ShareCard {
                tier_name: c.tier_name.clone(),
                prize_name: c.prize_name.clone(),
                rarity: c.rarity.clamp(0, 3) as u8,
                stars: c.stars.max(1) as usize,
                color: c.color.map_or_else(
                    || TIER_COLORS[(c.tier_at.max(0) as usize).min(TIER_COLORS.len() - 1)],
                    |color| color as u32 & 0xff_ffff,
                ),
                outcome: c.outcome(),
                guaranteed: c.guaranteed,
            })
            .collect(),
    }
}

/// A pull just made, ranked by its banner as the server ranks a shared one.
pub fn cards_of_pull(pull: &pb::GachaPull, banner: &pb::GachaBanner) -> ShareCards {
    ShareCards {
        banner_name: pull.banner_name.clone(),
        cards: pull
            .results
            .iter()
            .map(|r| {
                let at = banner.tiers.iter().position(|t| t.id == r.tier_id);
                ShareCard {
                    tier_name: r.tier_name.clone(),
                    prize_name: r.prize_name.clone(),
                    rarity: eco::rarity(banner, &r.tier_id),
                    stars: eco::stars(banner, &r.tier_id),
                    color: at.map_or_else(commonest, |at| eco::tier_color(&banner.tiers[at], at)),
                    outcome: r.outcome(),
                    guaranteed: r.guaranteed,
                }
            })
            .collect(),
    }
}

/// The card the most is made of: the rarest, the first of them.
pub fn best_card(cards: &[ShareCard]) -> Option<&ShareCard> {
    cards.iter().fold(None, |best: Option<&ShareCard>, c| match best {
        Some(b) if c.rarity <= b.rarity => Some(b),
        _ => Some(c),
    })
}

/// What the chip under a card says, if anything: how it came out, else that pity made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Refunded,
    Duplicate,
    Guaranteed,
}

pub fn chip(card: &ShareCard) -> Option<Chip> {
    match card.outcome {
        pb::GachaOutcome::Refunded => Some(Chip::Refunded),
        pb::GachaOutcome::Duplicate => Some(Chip::Duplicate),
        _ => card.guaranteed.then_some(Chip::Guaranteed),
    }
}

// The picture.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// Words drawn on the picture.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageText {
    pub x: f64,
    pub y: f64,
    pub size: u32,
    pub weight: u32,
    /// #rrggbb.
    pub color: String,
    pub opacity: f64,
    pub anchor: Anchor,
    pub text: String,
}

/// The picture: `shapes` is an SVG without words (the web draws it, then writes the words in its font).
#[derive(Debug, Clone, PartialEq)]
pub struct ShareImage {
    pub width: u32,
    pub height: u32,
    pub shapes: String,
    pub texts: Vec<ImageText>,
}

/// Its words, in the reader's language.
pub struct ImageLabels<'a> {
    /// Under the banner's name: who pulled, and how many.
    pub subtitle: String,
    /// The chip under a card.
    pub chip: &'a dyn Fn(Chip) -> String,
}

/// The font the words are in: the apps' own, as the web names it...
pub const IMAGE_FONT: &str = "M PLUS Rounded 1c";
/// ...and as its files do (`ui::theme::FONT`).
const FILE_FONT: &str = "Rounded Mplus 1c";

const PAD: f64 = 40.0;
const HEAD: f64 = 120.0;
const CARD_W: f64 = 160.0;
const CARD_H: f64 = 216.0;
const GAP: f64 = 16.0;
const FOOT: f64 = 40.0;
const COLS: usize = 5;
const NIGHT: &str = "#0b0a14";
const FACE: &str = "#15131f";
const WHITE: &str = "#ffffff";

/// A number as the SVG writes it: to two places, as short as it goes (as JavaScript writes it).
pub fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { "0".into() } else { format!("{r}") }
}

pub fn hex(color: u32) -> String {
    format!("#{:06x}", color & 0xff_ffff)
}

pub fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// At most `most` characters, the last an ellipsis when it's cut.
pub fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(most - 1).collect();
        cut.push('…');
        cut
    }
}

/// A five-pointed star around (cx, cy).
fn star(cx: f64, cy: f64, outer: f64, inner: f64) -> String {
    (0..10)
        .map(|i| {
            let r = if i % 2 == 0 { outer } else { inner };
            let a = PI / 5.0 * f64::from(i) - PI / 2.0;
            format!("{},{}", num(cx + r * a.cos()), num(cy + r * a.sin()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The four-pointed sparkle in the middle of a card.
fn sparkle(cx: f64, cy: f64, r: f64) -> String {
    let k = r * 0.28;
    let n = num;
    format!(
        "M{} {}Q{} {} {} {}Q{} {} {} {}Q{} {} {} {}Q{} {} {} {}Z",
        n(cx),
        n(cy - r),
        n(cx + k),
        n(cy - k),
        n(cx + r),
        n(cy),
        n(cx + k),
        n(cy + k),
        n(cx),
        n(cy + r),
        n(cx - k),
        n(cy + k),
        n(cx - r),
        n(cy),
        n(cx - k),
        n(cy - k),
        n(cx),
        n(cy - r),
    )
}

/// Lays out the picture of a pull's cards.
pub fn share_image(pulled: &ShareCards, labels: &ImageLabels) -> ShareImage {
    let n = pulled.cards.len();
    let cols = n.clamp(1, COLS);
    let rows = n.div_ceil(cols).max(1);
    let width = (PAD * 2.0 + cols as f64 * CARD_W + (cols as f64 - 1.0) * GAP).max(480.0);
    let height = HEAD + rows as f64 * CARD_H + (rows as f64 - 1.0) * GAP + FOOT + PAD / 2.0;
    let glow = hex(best_card(&pulled.cards).map_or(TIER_COLORS[0], |c| c.color));
    let mut defs = vec![format!(
        r#"<radialGradient id="glow" cx="50%" cy="0%" r="75%"><stop offset="0" stop-color="{glow}" stop-opacity="0.45"/><stop offset="1" stop-color="{glow}" stop-opacity="0"/></radialGradient>"#
    )];
    let (w, h) = (num(width), num(height));
    let mut body = vec![
        format!(r#"<rect width="{w}" height="{h}" rx="32" fill="{NIGHT}"/>"#),
        format!(r#"<rect width="{w}" height="{h}" rx="32" fill="url(#glow)"/>"#),
    ];
    let text = |x: f64, y: f64, size: u32, weight: u32, color: &str, opacity: f64, anchor: Anchor, text: String| {
        ImageText { x, y, size, weight, color: color.to_owned(), opacity, anchor, text }
    };
    let mut texts = vec![
        text(PAD, 66.0, 32, 800, WHITE, 1.0, Anchor::Start, clip(&pulled.banner_name, 40)),
        text(PAD, 98.0, 17, 400, WHITE, 0.65, Anchor::Start, labels.subtitle.clone()),
    ];
    for (i, card) in pulled.cards.iter().enumerate() {
        let row = i / cols;
        let in_row = cols.min(n - row * cols) as f64;
        let row_w = in_row * CARD_W + (in_row - 1.0) * GAP;
        let x = (width - row_w) / 2.0 + (i % cols) as f64 * (CARD_W + GAP);
        let y = HEAD + row as f64 * (CARD_H + GAP);
        let cx = x + CARD_W / 2.0;
        let color = hex(card.color);
        let (cw, ch) = (num(CARD_W), num(CARD_H));
        defs.push(format!(
            r#"<linearGradient id="c{i}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{color}" stop-opacity="0.45"/><stop offset="0.6" stop-color="{color}" stop-opacity="0"/></linearGradient>"#
        ));
        if card.rarity >= 3 {
            body.push(format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="24" fill="{color}" fill-opacity="0.28"/>"#,
                num(x - 6.0),
                num(y - 6.0),
                num(CARD_W + 12.0),
                num(CARD_H + 12.0)
            ));
        }
        body.push(format!(r#"<rect x="{}" y="{}" width="{cw}" height="{ch}" rx="18" fill="{FACE}"/>"#, num(x), num(y)));
        body.push(format!(
            r#"<rect x="{}" y="{}" width="{cw}" height="{ch}" rx="18" fill="url(#c{i})" stroke="{color}" stroke-width="3"/>"#,
            num(x),
            num(y)
        ));
        let count = card.stars.clamp(1, 5);
        for s in 0..count {
            let sx = cx + (s as f64 - (count as f64 - 1.0) / 2.0) * 18.0;
            body.push(format!(r#"<polygon points="{}" fill="{color}"/>"#, star(sx, y + 26.0, 8.0, 3.4)));
        }
        body.push(format!(
            r#"<circle cx="{}" cy="{}" r="34" fill="{color}" fill-opacity="0.22"/>"#,
            num(cx),
            num(y + 88.0)
        ));
        let r = if card.rarity >= 2 { 22.0 } else { 16.0 };
        body.push(format!(r#"<path d="{}" fill="{color}"/>"#, sparkle(cx, y + 88.0, r)));
        texts.push(text(cx, y + 156.0, 15, 700, WHITE, 1.0, Anchor::Middle, clip(&card.prize_name, 17)));
        texts.push(text(cx, y + 178.0, 12, 800, &color, 1.0, Anchor::Middle, clip(&card.tier_name.to_uppercase(), 18)));
        if let Some(c) = chip(card) {
            texts.push(text(cx, y + 200.0, 11, 700, WHITE, 0.7, Anchor::Middle, clip(&(labels.chip)(c), 20)));
        }
    }
    texts.push(text(width - PAD, height - 24.0, 14, 800, WHITE, 0.4, Anchor::End, "fuwa".into()));
    let shapes = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><defs>{}</defs>{}</svg>"#,
        defs.concat(),
        body.concat()
    );
    ShareImage { width: width as u32, height: height as u32, shapes, texts }
}

/// The whole picture as one SVG, words and all.
pub fn image_svg(image: &ShareImage) -> String {
    let words: String = image
        .texts
        .iter()
        .map(|t| {
            let anchor = match t.anchor {
                Anchor::Start => "start",
                Anchor::Middle => "middle",
                Anchor::End => "end",
            };
            let opacity = if t.opacity < 1.0 { format!(r#" fill-opacity="{}""#, num(t.opacity)) } else { String::new() };
            format!(
                r#"<text x="{}" y="{}" font-family="'{IMAGE_FONT}', '{}', sans-serif" font-size="{}" font-weight="{}" fill="{}"{opacity} text-anchor="{anchor}">{}</text>"#,
                num(t.x),
                num(t.y),
                FILE_FONT,
                t.size,
                t.weight,
                t.color,
                escape_xml(&t.text)
            )
        })
        .collect();
    let shapes = image.shapes.strip_suffix("</svg>").unwrap_or(&image.shapes);
    format!("{shapes}{words}</svg>")
}

/// The fonts the picture's words are drawn in: the app's own, then the system's for anything else.
fn fonts() -> Arc<resvg::usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            for font in [
                &include_bytes!("../../assets/fonts/MPLUSRounded1c-Regular.ttf")[..],
                &include_bytes!("../../assets/fonts/MPLUSRounded1c-Bold.ttf")[..],
                &include_bytes!("../../assets/fonts/MPLUSRounded1c-ExtraBold.ttf")[..],
            ] {
                db.load_font_data(font.to_vec());
            }
            db.load_system_fonts();
            db.set_sans_serif_family(FILE_FONT);
            Arc::new(db)
        })
        .clone()
}

/// The picture as a PNG, `scale` times its layout's size.
pub fn render_png(image: &ShareImage, scale: f32) -> Result<Vec<u8>, String> {
    let options = resvg::usvg::Options { fontdb: fonts(), ..Default::default() };
    let tree = resvg::usvg::Tree::from_str(&image_svg(image), &options).map_err(|e| e.to_string())?;
    let (w, h) = ((image.width as f32 * scale).ceil() as u32, (image.height as f32 * scale).ceil() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("too big")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// A file name for a pull's picture: the banner's name, kept to safe characters.
pub fn image_name(banner_name: &str) -> String {
    let mut base = String::new();
    for c in banner_name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            base.push(c);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = base.trim_end_matches('-');
    format!("{}-pull.png", if base.is_empty() { "gacha" } else { base })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(
        tier: &str,
        prize: &str,
        rarity: u8,
        stars: usize,
        color: u32,
        outcome: pb::GachaOutcome,
        guaranteed: bool,
    ) -> ShareCard {
        ShareCard { tier_name: tier.into(), prize_name: prize.into(), rarity, stars, color, outcome, guaranteed }
    }

    /// The web's FIXTURE in gacha-share.test.ts.
    fn fixture() -> ShareCards {
        use pb::GachaOutcome::{Refunded, Won};
        ShareCards {
            banner_name: "Sakura <Festival> & Friends".into(),
            cards: vec![
                card("Common", "50 Sakura Petals", 0, 1, 0x10b981, Won, false),
                card("Rare", "Custom Emoji Slot Deluxe", 1, 2, 0x3b82f6, Won, false),
                card("Epic", "Night Owl", 2, 3, 0xa855f7, Refunded, true),
                card("Legendary", "Radiant", 3, 4, 0xf59e0b, Won, false),
                card("Common", "Pin Request", 0, 1, 0x10b981, Won, false),
                card("Rare", "200 Tokens", 1, 2, 0x3b82f6, Won, false),
            ],
        }
    }

    fn chip_words(c: Chip) -> String {
        match c {
            Chip::Refunded => "Refunded",
            Chip::Duplicate => "Already had it",
            Chip::Guaranteed => "Guaranteed",
        }
        .into()
    }

    #[test]
    fn the_picture_is_laid_out_as_the_web_lays_it_out() {
        let labels = ImageLabels { subtitle: "alice · 6 pulls".into(), chip: &chip_words };
        let image = share_image(&fixture(), &labels);
        assert_eq!((image.width, image.height), (40 * 2 + 5 * 160 + 4 * 16, 120 + 2 * 216 + 16 + 40 + 20));
        let expected = include_str!("../../../web/src/lib/gacha-share.test.svg").trim();
        assert_eq!(image_svg(&image), expected);
        let one = ShareCards { banner_name: "x".into(), cards: fixture().cards[..1].to_vec() };
        assert_eq!(share_image(&one, &labels).width, 480);
        // And it draws.
        let png = render_png(&image, 1.0).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn the_pictures_little_rules() {
        assert_eq!([num(1.005), num(12.0), num(-0.001), num(3.456)], ["1", "12", "0", "3.46"]);
        assert_eq!(escape_xml(r#"<a & 'b'>""#), "&lt;a &amp; &apos;b&apos;&gt;&quot;");
        assert_eq!(clip("Custom Emoji Slot Deluxe", 16), "Custom Emoji Sl…");
        assert_eq!(clip("Holo", 16), "Holo");
        assert_eq!(image_name("Sakura Festival ✿ 2026"), "sakura-festival-2026-pull.png");
        assert_eq!(image_name("✿"), "gacha-pull.png");
    }

    #[test]
    fn a_pull_and_a_shared_one_come_out_as_the_same_cards() {
        let tier = |id: &str, color: Option<i32>| pb::GachaTier { id: id.into(), name: id.into(), color, rate: 1 };
        let banner = pb::GachaBanner {
            tiers: vec![tier("ssr", None), tier("sr", Some(0x123456)), tier("r", None)],
            pity_tier_id: "sr".into(),
            ..Default::default()
        };
        let result = |tier_id: &str, outcome: pb::GachaOutcome, guaranteed: bool| pb::GachaResult {
            tier_id: tier_id.into(),
            tier_name: tier_id.into(),
            prize_name: format!("{tier_id} prize"),
            outcome: outcome as i32,
            guaranteed,
            ..Default::default()
        };
        let pull = pb::GachaPull {
            banner_name: "B".into(),
            results: vec![
                result("r", pb::GachaOutcome::Won, false),
                result("ssr", pb::GachaOutcome::Won, true),
                result("sr", pb::GachaOutcome::Refunded, false),
            ],
            ..Default::default()
        };
        let from_pull = cards_of_pull(&pull, &banner);
        let ranks: Vec<_> = from_pull.cards.iter().map(|c| (c.rarity, c.stars, c.color)).collect();
        assert_eq!(ranks, [(0, 1, 0x3b82f6), (3, 3, 0xf59e0b), (2, 2, 0x123456)]);
        let shared = |c: &ShareCard, tier_at: i32, color: Option<i32>| pb::GachaShareCard {
            tier_name: c.tier_name.clone(),
            prize_name: c.prize_name.clone(),
            rarity: i32::from(c.rarity),
            stars: c.stars as i32,
            tier_at,
            color,
            outcome: c.outcome as i32,
            guaranteed: c.guaranteed,
            item_id: String::new(),
        };
        let c = &from_pull.cards;
        let share = pb::GachaShare {
            banner_name: "B".into(),
            cards: vec![shared(&c[0], 2, None), shared(&c[1], 0, None), shared(&c[2], 1, Some(0x123456))],
            ..Default::default()
        };
        assert_eq!(cards_of_share(&share), from_pull);
        assert_eq!(best_card(&from_pull.cards).unwrap().prize_name, "ssr prize");
        let chips: Vec<_> = from_pull.cards.iter().map(chip).collect();
        assert_eq!(chips, [None, Some(Chip::Guaranteed), Some(Chip::Refunded)]);
    }
}
