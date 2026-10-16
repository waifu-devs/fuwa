//! The Economy page's gacha banners (docs/economy.md), like the web's
//! `settings/server/GachaBanners.tsx`: each banner's name, art, price and
//! multi-pull, its rarity tiers (rarest first) and their rates, the prizes in
//! each with their weights and each one's chance as it stands, pity, what
//! duplicates do, the pull cap, when it's open and whether members see it.
//! It opens in place like a currency or listing, and saves whole from the
//! floating bar; the draft and its checks are `BannerDraft` in
//! core/economy.rs.

use super::*;
use crate::core::economy::{
    BANNER_DESCRIPTION_MAX, BannerProblem, Openness, PRIZES_MAX, TIER_SWATCHES, TIERS_MAX, WHOLE, prize_listings,
    tier_color,
};
use crate::pb::GachaDuplicates as Dup;
use crate::ui::widgets::picture;
use gpui_kit::ObjectFit;
use gpui_kit::StyledImage as _;

/// What a box in the banner editor holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Field {
    Name,
    Price,
    Multi,
    MultiPrice,
    Pity,
    Refund,
    MaxPulls,
    StartsAt,
    EndsAt,
    Position,
    TierName(String),
    TierRate(String),
    PrizeAmount(String),
    PrizeWeight(String),
}

/// The banner editor's boxes, made for the banner open (and each tier and
/// prize as it's added), and what's open in its pickers.
#[derive(Default)]
pub(super) struct Boxes {
    boxes: HashMap<Field, (Entity<InputState>, Subscription)>,
    description: Option<(Entity<TextareaState>, Subscription)>,
    /// The tier whose color swatches are open.
    coloring: Option<String>,
    /// The tier whose "Add a prize" choices are open.
    adding: Option<String>,
    /// New prizes made so far, for their ids.
    made: usize,
    /// The art being uploaded.
    uploading: bool,
}

/// What a field's box starts with, from the draft.
fn field_text(draft: &BannerDraft, field: &Field) -> String {
    let tier = |id: &str| draft.tiers.iter().find(|t| t.id == id);
    let prize = |id: &str| draft.prizes.iter().find(|p| p.id == id);
    match field {
        Field::Name => draft.name.clone(),
        Field::Price => draft.price.clone(),
        Field::Multi => draft.multi.clone(),
        Field::MultiPrice => draft.multi_price.clone(),
        Field::Pity => draft.pity_pulls.clone(),
        Field::Refund => draft.refund_amount.clone(),
        Field::MaxPulls => draft.max_pulls.clone(),
        Field::StartsAt => draft.starts_at.clone(),
        Field::EndsAt => draft.ends_at.clone(),
        Field::Position => draft.position.clone(),
        Field::TierName(id) => tier(id).map(|t| t.name.clone()).unwrap_or_default(),
        Field::TierRate(id) => tier(id).map(|t| t.rate.clone()).unwrap_or_default(),
        Field::PrizeAmount(id) => prize(id).map(|p| p.amount.clone()).unwrap_or_default(),
        Field::PrizeWeight(id) => prize(id).map(|p| p.weight.clone()).unwrap_or_default(),
    }
}

/// What an empty box shows.
fn field_placeholder(field: &Field) -> String {
    match field {
        Field::Name => t("serversettings.economy.gacha.namePlaceholder"),
        Field::Multi | Field::Pity => t("serversettings.economy.gacha.none"),
        Field::MaxPulls => t("settings.controls.noLimit"),
        Field::StartsAt | Field::EndsAt => "2026-12-24 18:00".to_owned(),
        Field::TierName(_) => t("serversettings.economy.gacha.tierName"),
        _ => String::new(),
    }
}

/// The color a tier shows: its own, or the default for its place.
fn tier_hsla(color: Option<u32>, index: usize) -> Hsla {
    gpui_kit::rgb(tier_color(&pb::GachaTier { color: color.map(|c| c as i32), ..Default::default() }, index)).into()
}

/// A banner's lead color: its rarest tier with a rate.
fn glow(b: &pb::GachaBanner) -> Hsla {
    let at = b.tiers.iter().position(|t| t.rate > 0).unwrap_or(0);
    tier_hsla(b.tiers.get(at).and_then(|t| t.color).map(|c| c as u32), at)
}

/// A small upper-case tag on a banner's card.
fn tag(text: String, p: &Palette) -> Div {
    div()
        .flex_none()
        .px(px(4.0))
        .rounded(corner(4.0))
        .bg(p.muted)
        .text_size(px(10.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(text.to_uppercase())
}

/// A field's name, the box or picker under it, and a hint, red when it's what's wrong.
fn field(name: String, wrong: bool, body: impl IntoElement, hint: Option<String>, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(div().text_xs().font_weight(FontWeight::BOLD).when(wrong, |el| el.text_color(p.destructive)).child(name))
        .child(body)
        .when_some(hint, |el, h| el.child(div().text_xs().text_color(p.muted_foreground).child(h)))
}

impl ServerSettingsView {
    fn banner_draft(&mut self) -> Option<&mut BannerDraft> {
        match &mut self.economy.editor {
            Some(Editor::Banner { draft, .. }) => Some(draft),
            _ => None,
        }
    }

    /// Opens a banner in the editor, or a new one (in the first currency, last in line).
    pub(super) fn edit_banner(&mut self, banner: Option<pb::GachaBanner>, window: &mut Window, cx: &mut Context<Self>) {
        let (currency, count) = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.economy(&self.server))
                .map(|e| (e.currencies.first().map(|c| c.id.clone()).unwrap_or_default(), e.banners.len()))
                .unwrap_or_default()
        });
        let before = match &banner {
            Some(b) => BannerDraft::of(b),
            None => BannerDraft::new(
                &currency,
                count,
                [
                    t("serversettings.economy.gacha.starter.legendary"),
                    t("serversettings.economy.gacha.starter.rare"),
                    t("serversettings.economy.gacha.starter.common"),
                ],
            ),
        };
        self.economy.editor = Some(Editor::Banner {
            id: banner.map(|b| b.id),
            before: Box::new(before.clone()),
            draft: Box::new(before),
        });
        self.economy.asking = None;
        self.economy.banner = Boxes::default();
        self.error = None;
        self.banner_boxes(window, cx);
        if let Some((name, _)) = self.economy.banner.boxes.get(&Field::Name) {
            name.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }

    /// Makes the boxes the open banner needs and doesn't have yet, filled from its draft.
    fn banner_boxes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Editor::Banner { draft, .. }) = &self.economy.editor else { return };
        let mut fields = vec![
            Field::Name,
            Field::Price,
            Field::Multi,
            Field::MultiPrice,
            Field::Pity,
            Field::Refund,
            Field::MaxPulls,
            Field::StartsAt,
            Field::EndsAt,
            Field::Position,
        ];
        for tier in &draft.tiers {
            fields.push(Field::TierName(tier.id.clone()));
            fields.push(Field::TierRate(tier.id.clone()));
        }
        for prize in &draft.prizes {
            fields.push(Field::PrizeAmount(prize.id.clone()));
            fields.push(Field::PrizeWeight(prize.id.clone()));
        }
        let draft = draft.clone();
        // Boxes of tiers and prizes taken out go with them.
        self.economy.banner.boxes.retain(|f, _| fields.contains(f));
        for f in fields {
            if self.economy.banner.boxes.contains_key(&f) {
                continue;
            }
            let (value, placeholder) = (field_text(&draft, &f), field_placeholder(&f));
            let state = cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(placeholder);
                s.set_value(value, window, cx);
                s
            });
            let at = f.clone();
            let sub = cx.subscribe(&state, move |this: &mut Self, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    this.banner_typed(&at, value, cx);
                }
            });
            self.economy.banner.boxes.insert(f, (state, sub));
        }
        if self.economy.banner.description.is_none() {
            let state = cx.new(|cx| {
                let mut s = TextareaState::new(window, cx)
                    .auto_grow(2, 6)
                    .placeholder(t("serversettings.economy.gacha.descriptionPlaceholder"));
                s.set_value(draft.description.clone(), window, cx);
                s
            });
            let sub = cx.subscribe(&state, |this: &mut Self, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    if let Some(d) = this.banner_draft() {
                        d.description = value;
                    }
                    this.error = None;
                    cx.notify();
                }
            });
            self.economy.banner.description = Some((state, sub));
        }
    }

    /// Something was typed in one of the banner's boxes: the draft follows it.
    fn banner_typed(&mut self, field: &Field, value: String, cx: &mut Context<Self>) {
        let Some(d) = self.banner_draft() else { return };
        match field {
            Field::Name => d.name = value,
            Field::Price => d.price = value,
            Field::Multi => d.multi = value,
            Field::MultiPrice => d.multi_price = value,
            Field::Pity => d.pity_pulls = value,
            Field::Refund => d.refund_amount = value,
            Field::MaxPulls => d.max_pulls = value,
            Field::StartsAt => d.starts_at = value,
            Field::EndsAt => d.ends_at = value,
            Field::Position => d.position = value,
            Field::TierName(id) | Field::TierRate(id) => {
                if let Some(t) = d.tiers.iter_mut().find(|t| t.id == *id) {
                    if matches!(field, Field::TierName(_)) {
                        t.name = value;
                    } else {
                        t.rate = value;
                    }
                }
            }
            Field::PrizeAmount(id) | Field::PrizeWeight(id) => {
                if let Some(p) = d.prizes.iter_mut().find(|p| p.id == *id) {
                    if matches!(field, Field::PrizeAmount(_)) {
                        p.amount = value;
                    } else {
                        p.weight = value;
                    }
                }
            }
        }
        self.error = None;
        cx.notify();
    }

    /// Changes the open banner's draft from a picker or button, then makes any boxes it now needs.
    fn change_banner(&mut self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&mut BannerDraft)) {
        if let Some(d) = self.banner_draft() {
            f(d);
        }
        self.error = None;
        self.banner_boxes(window, cx);
        cx.notify();
    }

    /// Clears a box (the pull cap's "No limit").
    fn clear_box(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.economy.banner.boxes.get(&field).map(|(s, _)| s.clone()) {
            state.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.banner_typed(&field, String::new(), cx);
    }

    /// Asks the system for the banner's box art and uploads it for the server.
    fn pick_banner_art(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.economy.banner.uploading {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("serversettings.economy.gacha.artUpload").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.upload_banner_art(path, cx));
        })
        .detach();
    }

    fn upload_banner_art(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(mime) = crate::core::account::picture_type(&name) else {
            self.error = Some(t("serversettings.economy.gacha.artType"));
            cx.notify();
            return;
        };
        self.economy.banner.uploading = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move {
                let bytes = crate::core::account::read_picture(&path).await?;
                core.upload_picture_for(&key, &sid, pb::MediaPurpose::ShopArt, mime, bytes).await
            },
            |this, result, cx| {
                this.economy.banner.uploading = false;
                match result {
                    Ok(url) if !url.is_empty() => {
                        if let Some(d) = this.banner_draft() {
                            d.art_url = url;
                        }
                    }
                    Ok(_) => this.error = Some(t("serversettings.economy.gacha.artFailed")),
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// The banners section: its heading with New banner, the editor when a
    /// new one's open, and a card per banner (or the editor on the one open).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn banners_section(
        &mut self,
        economy: &eco::Economy,
        emojis: &[pb::Emoji],
        roles: &[pb::Role],
        items: &[pb::ProfileItem],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let new_open = matches!(self.economy.editor, Some(Editor::Banner { id: None, .. }));
        let can_make = !economy.currencies.is_empty();
        let mut col = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(div().flex_1().child(super::super::pages::heading(
                    &t("serversettings.economy.gacha.title"),
                    Some(&t("serversettings.economy.gacha.intro")),
                    p,
                )))
                .child(
                    super::super::pages::act(
                        "eco-new-banner",
                        t("serversettings.economy.gacha.new"),
                        icon("plus").size(px(16.0)),
                        Look::Primary,
                        p,
                    )
                    .when(new_open || !can_make, |el| el.opacity(0.5))
                    .when(!new_open && can_make, |el| {
                        el.on_click(cx.listener(|this, _, window, cx| this.edit_banner(None, window, cx)))
                    }),
                ),
        );
        if new_open {
            col = col.child(self.banner_editor(None, economy, emojis, roles, items, p, window, cx));
        }
        if economy.banners.is_empty() && !new_open {
            let text = if can_make {
                format!("{} {}", t("serversettings.economy.gacha.none"), t("serversettings.economy.gacha.noneHint"))
            } else {
                t("desktop.economy.listing.needCurrency")
            };
            col = col.child(empty_card("dices", &text, p));
        }
        let now = now_ms();
        for (n, b) in economy.banners.iter().enumerate() {
            let editing = matches!(&self.economy.editor, Some(Editor::Banner { id: Some(id), .. }) if *id == b.id);
            let card = if editing {
                self.banner_editor(Some(b), economy, emojis, roles, items, p, window, cx)
            } else {
                self.banner_card(b, economy, emojis, now, p, window, cx)
            };
            col = col.child(motion::rise(
                div().child(card),
                SharedString::from(format!("eco-b-in-{}", b.id)),
                Duration::from_millis((n.min(8) * 30) as u64),
                10.0,
            ));
        }
        col.into_any_element()
    }

    /// A banner, closed: its art (or a die in its color), name, whether members
    /// see it now, its price and how many tiers and prizes, Edit and Delete.
    #[allow(clippy::too_many_arguments)]
    fn banner_card(
        &self,
        b: &pb::GachaBanner,
        economy: &eco::Economy,
        emojis: &[pb::Emoji],
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let currency = economy.currency(&b.currency_id).cloned().unwrap_or_default();
        let asking = self.economy.asking.as_deref() == Some(b.id.as_str());
        let deleting = self.economy.deleting.as_deref() == Some(b.id.as_str());
        let tint = glow(b);
        let art: AnyElement = if b.art_url.is_empty() {
            div()
                .size(px(40.0))
                .flex_none()
                .rounded(corner(16.0))
                .bg(tint)
                .text_color(gpui_kit::rgb(0xffffff))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("dices").size(px(20.0)))
                .into_any_element()
        } else {
            picture(b.art_url.clone()).size(px(40.0)).flex_none().object_fit(ObjectFit::Contain).into_any_element()
        };
        let mut name_row = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(div().font_weight(FontWeight::BOLD).truncate().child(b.name.clone()));
        if !b.available {
            name_row = name_row.child(tag(t("serversettings.economy.gacha.hidden"), p));
        } else {
            match eco::openness(b, now) {
                Openness::Soon(_) => name_row = name_row.child(tag(t("serversettings.economy.gacha.soon"), p)),
                Openness::Closed => name_row = name_row.child(tag(t("serversettings.economy.gacha.closed"), p)),
                Openness::Open(_) => {}
            }
        }
        let facts = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.0))
            .text_xs()
            .text_color(p.muted_foreground)
            .child(money(&currency, emojis, b.pull_price, 13.0))
            .child(t_with("serversettings.economy.gacha.tierCount", &[("count", Arg::Num(b.tiers.len() as i64))]))
            .child(t_with("serversettings.economy.gacha.prizeCount", &[("count", Arg::Num(b.prizes.len() as i64))]));
        let (edit, del) = (b.clone(), b.id.clone());
        let hover = alpha(p.primary, 0.3);
        div()
            .id(SharedString::from(format!("eco-b-{}", b.id)))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .hover(move |s| s.border_color(hover))
            .when(!b.available, |el| el.opacity(0.75))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(art)
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.0)).child(name_row).child(facts))
                    .when(deleting, |el| el.child(spinner(format!("eco-bdel-{}", b.id), 16.0, window)))
                    .child(
                        soft_button(SharedString::from(format!("eco-bedit-{}", b.id)), t("desktop.economy.edit"), p)
                            .on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.edit_banner(Some(edit.clone()), window, cx)
                                }),
                            ),
                    )
                    .child(icon_button(SharedString::from(format!("eco-bdel-btn-{}", b.id)), "trash", p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.economy.asking = Some(del.clone());
                            cx.notify();
                        }),
                    )),
            )
            .when(asking, |el| {
                el.child(self.economy_asking(
                    &b.id,
                    format!(
                        "{} {}",
                        t_with("serversettings.economy.gacha.deleteTitle", &[("name", Arg::Str(&b.name))]),
                        t("serversettings.economy.gacha.deleteBody")
                    ),
                    Thing::Banner,
                    p,
                    cx,
                ))
            })
            .into_any_element()
    }

    /// One of the banner's boxes, `w` wide (None to fill).
    fn banner_box(&self, field: &Field, w: Option<f32>, p: &Palette, window: &Window, cx: &gpui_kit::App) -> Div {
        let Some((state, _)) = self.economy.banner.boxes.get(field) else { return div() };
        div().when_some(w, |el, w| el.w(px(w)).flex_none()).when(w.is_none(), |el| el.flex_1().min_w_0()).child(boxed(
            Input::new(state).appearance(false),
            36.0,
            focused(state, window, cx),
            p,
        ))
    }

    /// The banner open in the editor.
    #[allow(clippy::too_many_arguments)]
    fn banner_editor(
        &mut self,
        banner: Option<&pb::GachaBanner>,
        economy: &eco::Economy,
        emojis: &[pb::Emoji],
        roles: &[pb::Role],
        items: &[pb::ProfileItem],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.banner_boxes(window, cx);
        let Some(Editor::Banner { draft, .. }) = &self.economy.editor else { return div().into_any_element() };
        let draft = (**draft).clone();
        let problem = draft.value(banner.map(|b| b.id.as_str()).unwrap_or_default(), &self.server).err();
        let wrong = |key: &str| problem.is_some_and(|pr| pr.key.ends_with(key));
        let currency = economy.currency(&draft.currency_id).cloned().unwrap_or_default();
        let mut form = div().flex().flex_col().gap(px(16.0));

        // Name and price.
        form = form.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(16.0))
                .child(div().flex_1().min_w(px(220.0)).child(field(
                    t("serversettings.economy.gacha.name"),
                    wrong("gacha.problem.name"),
                    self.banner_box(&Field::Name, None, p, window, cx),
                    None,
                    p,
                )))
                .child(field(
                    t("serversettings.economy.gacha.price"),
                    wrong("economy.problem.price"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(currency_icon(&currency, emojis, 18.0))
                        .child(self.banner_box(&Field::Price, Some(140.0), p, window, cx)),
                    None,
                    p,
                )),
        );
        // In which currency, when there's a choice.
        if economy.currencies.len() > 1 {
            let mut picks = div().flex().flex_wrap().gap(px(6.0));
            for c in &economy.currencies {
                let id = c.id.clone();
                picks = picks.child(
                    chip(SharedString::from(format!("eco-bcur-{}", c.id)), &c.name, draft.currency_id == c.id, p)
                        .child(div().ml(px(6.0)).child(currency_icon(c, emojis, 14.0)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let id = id.clone();
                            this.change_banner(window, cx, |d| d.currency_id = id);
                        })),
                );
            }
            form = form.child(field(t("chattools.economy.currency"), false, picks, None, p));
        }
        form = form.child(self.banner_art(&draft, &banner.map(glow).unwrap_or(p.primary.into()), p, window, cx));
        // Description.
        if let Some((state, _)) = &self.economy.banner.description {
            form = form.child(field(
                t("serversettings.economy.description"),
                false,
                crate::ui::instance_home::focus_ring(
                    div()
                        .w_full()
                        .min_h(px(62.0))
                        .px(px(2.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .text_sm()
                        .child(Textarea::new(state).appearance(false)),
                    focused(state, window, cx),
                    p,
                ),
                (draft.description.chars().count() > BANNER_DESCRIPTION_MAX)
                    .then(|| format!("{}/{BANNER_DESCRIPTION_MAX}", draft.description.chars().count())),
                p,
            ));
        }
        // Multi-pull.
        let multi_hint = draft
            .multi
            .trim()
            .parse::<i64>()
            .ok()
            .zip(draft.price.trim().parse::<i64>().ok())
            .map(|(m, price)| amount_text(m.saturating_mul(price)));
        form = form.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(16.0))
                .child(field(
                    t("serversettings.economy.gacha.multi"),
                    wrong("gacha.problem.multi"),
                    self.banner_box(&Field::Multi, Some(140.0), p, window, cx),
                    Some(t("serversettings.economy.gacha.multiHint")),
                    p,
                ))
                .when(!draft.multi.trim().is_empty(), |el| {
                    el.child(field(
                        t("serversettings.economy.gacha.multiPrice"),
                        wrong("gacha.problem.multiPrice"),
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(currency_icon(&currency, emojis, 18.0))
                            .child(self.banner_box(&Field::MultiPrice, Some(140.0), p, window, cx))
                            .when(draft.multi_price.trim().is_empty(), |el| {
                                el.when_some(multi_hint, |el, h| {
                                    el.child(div().text_sm().text_color(p.muted_foreground).child(h))
                                })
                            }),
                        Some(t("serversettings.economy.gacha.multiPriceHint")),
                        p,
                    ))
                }),
        );

        form = form.child(self.banner_tiers(&draft, problem, p, window, cx));
        form = form.child(self.banner_prizes(&draft, problem, economy, emojis, roles, items, p, window, cx));

        // Pity.
        let mut pity = div().flex().flex_wrap().gap(px(16.0)).child(field(
            t("serversettings.economy.gacha.pity"),
            wrong("gacha.problem.pity"),
            self.banner_box(&Field::Pity, Some(140.0), p, window, cx),
            Some(t("serversettings.economy.gacha.pityHint")),
            p,
        ));
        if !draft.pity_pulls.trim().is_empty() {
            let mut picks = div().flex().flex_wrap().gap(px(6.0));
            for (i, tier) in draft.tiers.iter().enumerate() {
                let id = tier.id.clone();
                let name = if tier.name.trim().is_empty() { tier.id.clone() } else { tier.name.clone() };
                picks = picks.child(
                    chip(SharedString::from(format!("eco-bpity-{}", tier.id)), &name, draft.pity_tier_id == tier.id, p)
                        .child(div().ml(px(6.0)).size(px(8.0)).rounded_full().bg(tier_hsla(tier.color, i)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let id = id.clone();
                            this.change_banner(window, cx, |d| d.pity_tier_id = id);
                        })),
                );
            }
            pity = pity.child(div().flex_1().min_w(px(220.0)).child(field(
                t("serversettings.economy.gacha.pityTier"),
                false,
                picks,
                None,
                p,
            )));
        }
        form = form.child(pity);

        // Duplicates.
        let mut dups = div().flex().flex_wrap().gap(px(6.0));
        for (dup, key) in [
            (Dup::Keep, "serversettings.economy.gacha.dupKeep"),
            (Dup::Refund, "serversettings.economy.gacha.dupRefund"),
            (Dup::SkipOwned, "serversettings.economy.gacha.dupSkip"),
        ] {
            dups = dups.child(
                chip(SharedString::from(format!("eco-bdup-{}", dup as i32)), &t(key), draft.duplicates == dup, p)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.change_banner(window, cx, |d| d.duplicates = dup)),
                    ),
            );
        }
        let dup_hint = match draft.duplicates {
            Dup::Refund => "serversettings.economy.gacha.dupRefundHint",
            Dup::SkipOwned => "serversettings.economy.gacha.dupSkipHint",
            _ => "serversettings.economy.gacha.dupKeepHint",
        };
        form = form.child(field(
            t("serversettings.economy.gacha.duplicates"),
            wrong("gacha.problem.refund"),
            dups,
            Some(t(dup_hint)),
            p,
        ));
        if draft.duplicates == Dup::Refund {
            let refund_in = economy.currency(&draft.refund_currency_id).cloned().unwrap_or_default();
            let mut row = div().flex().flex_wrap().gap(px(16.0)).child(field(
                t("serversettings.economy.gacha.refund"),
                wrong("gacha.problem.refund"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(currency_icon(&refund_in, emojis, 18.0))
                    .child(self.banner_box(&Field::Refund, Some(140.0), p, window, cx)),
                None,
                p,
            ));
            if economy.currencies.len() > 1 {
                let mut picks = div().flex().flex_wrap().gap(px(6.0));
                for c in &economy.currencies {
                    let id = c.id.clone();
                    picks = picks.child(
                        chip(
                            SharedString::from(format!("eco-brefund-{}", c.id)),
                            &c.name,
                            draft.refund_currency_id == c.id,
                            p,
                        )
                        .child(div().ml(px(6.0)).child(currency_icon(c, emojis, 14.0)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let id = id.clone();
                            this.change_banner(window, cx, |d| d.refund_currency_id = id);
                        })),
                    );
                }
                row = row.child(field(t("serversettings.economy.gacha.refundIn"), false, picks, None, p));
            }
            form = form.child(row);
        }

        // Limits and when it's open.
        let unlimited = draft.max_pulls.trim().is_empty();
        form = form.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(16.0))
                .child(field(
                    t("serversettings.economy.gacha.maxPulls"),
                    wrong("gacha.problem.maxPulls"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(self.banner_box(&Field::MaxPulls, Some(120.0), p, window, cx))
                        .child(
                            div()
                                .id("eco-bmax-none")
                                .size(px(36.0))
                                .flex_none()
                                .rounded(radius_xl())
                                .border_1()
                                .border_color(p.border)
                                .when(unlimited, |el| el.bg(p.secondary))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .tooltip(|window, cx| {
                                    crate::ui::overlay::Tip::new(t("settings.controls.noLimit")).build(window, cx)
                                })
                                .child(icon("infinity").size(px(16.0)))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.clear_box(Field::MaxPulls, window, cx)),
                                ),
                        ),
                    Some(t("serversettings.economy.gacha.maxPullsHint")),
                    p,
                ))
                .child(field(
                    t("serversettings.economy.gacha.startsAt"),
                    wrong("gacha.problem.dates"),
                    self.banner_box(&Field::StartsAt, Some(170.0), p, window, cx),
                    Some(t("serversettings.economy.gacha.timeHint")),
                    p,
                ))
                .child(field(
                    t("serversettings.economy.gacha.endsAt"),
                    wrong("gacha.problem.dates"),
                    self.banner_box(&Field::EndsAt, Some(170.0), p, window, cx),
                    None,
                    p,
                )),
        );
        form = form.child(field(
            t("serversettings.economy.position"),
            wrong("economy.problem.position"),
            self.banner_box(&Field::Position, Some(90.0), p, window, cx),
            Some(t("serversettings.economy.positionHint")),
            p,
        ));
        form = form.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(t("serversettings.economy.gacha.available")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.economy.gacha.availableHint")),
                        ),
                )
                .child(switch(
                    SharedString::from("eco-banner-available"),
                    draft.available,
                    false,
                    cx,
                    |this: &mut Self, on, cx| {
                        if let Some(d) = this.banner_draft() {
                            d.available = on;
                        }
                        cx.notify();
                    },
                )),
        );
        if let Some(problem) = problem {
            form = form.child(div().text_sm().text_color(p.muted_foreground).child(t(problem.key)));
        }
        editor_card(
            if banner.is_none() {
                t("serversettings.economy.gacha.new")
            } else {
                banner.map(|b| b.name.clone()).unwrap_or_default()
            },
            form,
            p,
        )
    }

    /// The box art: the picture (or an empty frame), Upload or Replace, and Remove.
    fn banner_art(
        &self,
        draft: &BannerDraft,
        tint: &Hsla,
        p: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let uploading = self.economy.banner.uploading;
        let has = !draft.art_url.is_empty();
        let tint = *tint;
        let frame = div()
            .id("eco-bart")
            .relative()
            .w(px(80.0))
            .h(px(96.0))
            .flex_none()
            .rounded(corner(16.0))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .map(|el| {
                if has {
                    el.bg(tint.opacity(0.18))
                        .child(picture(draft.art_url.clone()).w(px(80.0)).h(px(96.0)).object_fit(ObjectFit::Contain))
                } else {
                    el.border_2()
                        .border_dashed()
                        .border_color(p.border)
                        .text_color(p.muted_foreground)
                        .child(icon("image-up").size(px(24.0)))
                }
            })
            .when(uploading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(alpha(p.background, 0.7))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(spinner("eco-bart-spin", 18.0, window)),
                )
            })
            .on_click(cx.listener(|this, _, window, cx| this.pick_banner_art(window, cx)));
        let mut buttons = div().flex().gap(px(8.0)).child(
            soft_button(
                "eco-bart-upload",
                if has {
                    t("serversettings.economy.gacha.artReplace")
                } else {
                    t("serversettings.economy.gacha.artUpload")
                },
                p,
            )
            .when(uploading, |el| el.opacity(0.5))
            .on_click(cx.listener(|this, _, window, cx| this.pick_banner_art(window, cx))),
        );
        if has {
            buttons = buttons.child(
                soft_button("eco-bart-remove", t("serversettings.economy.gacha.artRemove"), p)
                    .text_color(p.destructive)
                    .when(uploading, |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.economy.banner.uploading {
                            this.change_banner(window, cx, |d| d.art_url.clear());
                        }
                    })),
            );
        }
        field(
            t("serversettings.economy.gacha.art"),
            false,
            div().flex().items_center().gap(px(12.0)).child(frame).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div().text_xs().text_color(p.muted_foreground).child(t("serversettings.economy.gacha.artHint")),
                    )
                    .child(buttons),
            ),
            None,
            p,
        )
    }

    /// The rarity tiers, rarest first: color, name, rate, Move up and Remove,
    /// with what the rates add up to.
    fn banner_tiers(
        &self,
        draft: &BannerDraft,
        problem: Option<BannerProblem>,
        p: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let total = draft.rates_total();
        let whole = total == Some(WHOLE);
        let ok: Hsla = gpui_kit::rgb(0x10b981).into();
        let wrong_at = problem.filter(BannerProblem::tiers).map(|pr| pr.index);
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (i, tier) in draft.tiers.iter().enumerate() {
            let color = tier_hsla(tier.color, i);
            let (up, gone, paint) = (i, i, tier.id.clone());
            let coloring = self.economy.banner.coloring.as_deref() == Some(tier.id.as_str());
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .p(px(6.0))
                .rounded(corner(12.0))
                .border_1()
                .border_color(if wrong_at == Some(Some(i)) { p.destructive } else { p.border })
                .child(
                    div()
                        .id(SharedString::from(format!("eco-btier-color-{}", tier.id)))
                        .size(px(32.0))
                        .flex_none()
                        .rounded(corner(8.0))
                        .bg(color)
                        .cursor_pointer()
                        .tooltip(|window, cx| {
                            crate::ui::overlay::Tip::new(t("serversettings.economy.gacha.tierColor")).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let b = &mut this.economy.banner;
                            b.coloring =
                                if b.coloring.as_deref() == Some(paint.as_str()) { None } else { Some(paint.clone()) };
                            cx.notify();
                        })),
                )
                .child(self.banner_box(&Field::TierName(tier.id.clone()), None, p, window, cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(self.banner_box(&Field::TierRate(tier.id.clone()), Some(84.0), p, window, cx))
                        .child(div().text_xs().text_color(p.muted_foreground).child("%")),
                )
                .child(
                    icon_button(SharedString::from(format!("eco-btier-up-{}", tier.id)), "arrow-up", p)
                        .when(i == 0, |el| el.opacity(0.4))
                        .tooltip(|window, cx| {
                            crate::ui::overlay::Tip::new(t("serversettings.economy.gacha.rarer")).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.change_banner(window, cx, |d| d.raise_tier(up));
                        })),
                )
                .child(
                    icon_button(SharedString::from(format!("eco-btier-x-{}", tier.id)), "x", p)
                        .when(draft.tiers.len() <= 1, |el| el.opacity(0.4))
                        .tooltip(|window, cx| {
                            crate::ui::overlay::Tip::new(t("serversettings.economy.gacha.removeTier")).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.change_banner(window, cx, |d| d.remove_tier(gone));
                        })),
                );
            if coloring {
                let mut swatches = div().flex().flex_wrap().gap(px(6.0)).pl(px(46.0));
                for c in TIER_SWATCHES {
                    let id = tier.id.clone();
                    let on = tier.color == Some(c);
                    swatches = swatches.child(
                        div()
                            .id(SharedString::from(format!("eco-btier-sw-{}-{c:06x}", tier.id)))
                            .size(px(22.0))
                            .rounded_full()
                            .bg(gpui_kit::rgb(c))
                            .cursor_pointer()
                            .when(on, |el| el.border_2().border_color(p.foreground))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let id = id.clone();
                                this.economy.banner.coloring = None;
                                this.change_banner(window, cx, |d| {
                                    if let Some(t) = d.tiers.iter_mut().find(|t| t.id == id) {
                                        t.color = Some(c);
                                    }
                                });
                            })),
                    );
                }
                row = row.flex_wrap();
                list = list.child(div().flex().flex_col().gap(px(6.0)).child(row).child(swatches));
            } else {
                list = list.child(row);
            }
        }
        let total_text = t_with(
            "serversettings.economy.gacha.total",
            &[("total", Arg::Str(&total.map(eco::percent).unwrap_or_else(|| "?".to_owned())))],
        );
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .when(wrong_at.is_some(), |el| el.text_color(p.destructive))
                            .child(t("serversettings.economy.gacha.tiers")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if whole { ok } else { p.destructive.into() })
                            .child(icon(if whole { "circle-check" } else { "circle-x" }).size(px(14.0)))
                            .child(total_text),
                    ),
            )
            .child(div().text_xs().text_color(p.muted_foreground).child(t("serversettings.economy.gacha.tiersHint")))
            .child(list)
            .child(
                div().flex().child(
                    soft_button("eco-btier-add", t("serversettings.economy.gacha.addTier"), p)
                        .when(draft.tiers.len() >= TIERS_MAX, |el| el.opacity(0.5))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.change_banner(window, cx, BannerDraft::add_tier)),
                        ),
                ),
            )
    }

    /// The prizes under their tiers: each with what it is (a listing, or an
    /// amount of a currency), its weight, its chance per pull and Remove; then
    /// "Add a prize…" for the tier.
    #[allow(clippy::too_many_arguments)]
    fn banner_prizes(
        &self,
        draft: &BannerDraft,
        problem: Option<BannerProblem>,
        economy: &eco::Economy,
        emojis: &[pb::Emoji],
        roles: &[pb::Role],
        items: &[pb::ProfileItem],
        p: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let chances = draft.chances();
        let wrong_prize = problem.filter(BannerProblem::prize).and_then(|pr| pr.index);
        let empty_tier = problem.filter(BannerProblem::empty_tier).and_then(|pr| pr.index);
        let red = wrong_prize.is_some() || empty_tier.is_some();
        let listings = prize_listings(&economy.listings);
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .when(red, |el| el.text_color(p.destructive))
                    .child(t("serversettings.economy.gacha.prizes")),
            )
            .child(div().text_xs().text_color(p.muted_foreground).child(t("serversettings.economy.gacha.prizesHint")));
        for (ti, tier) in draft.tiers.iter().enumerate() {
            let tier_name = if tier.name.trim().is_empty() {
                t("serversettings.economy.gacha.unnamedTier")
            } else {
                tier.name.clone()
            };
            let mut group =
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .p(px(8.0))
                    .rounded(corner(16.0))
                    .border_1()
                    .border_dashed()
                    .border_color(if empty_tier == Some(ti) { p.destructive } else { p.border })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(4.0))
                            .text_sm()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(div().size(px(10.0)).rounded_full().bg(tier_hsla(tier.color, ti)))
                            .child(div().flex_1().min_w_0().truncate().child(tier_name))
                            .child(div().text_xs().text_color(p.muted_foreground).child(format!(
                                "{}%",
                                if tier.rate.trim().is_empty() { "0" } else { tier.rate.trim() }
                            ))),
                    );
            for (index, prize) in draft.prizes.iter().enumerate().filter(|(_, pr)| pr.tier_id == tier.id) {
                let what: AnyElement =
                    if !prize.listing_id.is_empty() {
                        let listing = economy.listing(&prize.listing_id);
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(div().truncate().child(match listing {
                                Some(l) => listing_name(l, roles, items),
                                None => t("chattools.economy.somethingFromShop"),
                            }))
                            .when(listing.is_some_and(|l| l.gacha_only), |el| {
                                el.child(icon("dices").size(px(14.0)).text_color(gpui_kit::rgb(0x8b5cf6)))
                            })
                            .into_any_element()
                    } else {
                        let mut what =
                            div().flex_1().min_w(px(160.0)).flex().flex_wrap().items_center().gap(px(6.0)).child(
                                self.banner_box(&Field::PrizeAmount(prize.id.clone()), Some(96.0), p, window, cx),
                            );
                        if economy.currencies.len() > 1 {
                            for c in &economy.currencies {
                                let (pid, cid) = (prize.id.clone(), c.id.clone());
                                what = what.child(
                                    chip(
                                        SharedString::from(format!("eco-bpcur-{}-{}", prize.id, c.id)),
                                        &c.name,
                                        prize.currency_id == c.id,
                                        p,
                                    )
                                    .h(px(28.0))
                                    .px(px(10.0))
                                    .child(div().ml(px(6.0)).child(currency_icon(c, emojis, 13.0)))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            let (pid, cid) = (pid.clone(), cid.clone());
                                            this.change_banner(window, cx, |d| {
                                                if let Some(pr) = d.prizes.iter_mut().find(|pr| pr.id == pid) {
                                                    pr.currency_id = cid;
                                                }
                                            });
                                        },
                                    )),
                                );
                            }
                        } else if let Some(c) = economy.currencies.first() {
                            what = what.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(currency_icon(c, emojis, 14.0))
                                    .child(c.name.clone()),
                            );
                        }
                        what.into_any_element()
                    };
                let gone = prize.id.clone();
                group = group.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .p(px(6.0))
                        .rounded(corner(12.0))
                        .bg(alpha(p.muted, 0.4))
                        .when(wrong_prize == Some(index), |el| el.border_2().border_color(p.destructive))
                        .child(what)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.economy.gacha.weight"))
                                .child(self.banner_box(
                                    &Field::PrizeWeight(prize.id.clone()),
                                    Some(80.0),
                                    p,
                                    window,
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("eco-bchance-{}", prize.id)))
                                .w(px(64.0))
                                .text_right()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .tooltip(|window, cx| {
                                    crate::ui::overlay::Tip::new(t("serversettings.economy.gacha.chance"))
                                        .build(window, cx)
                                })
                                .child(eco::percent(chances.get(&prize.id).copied().unwrap_or(0))),
                        )
                        .child(
                            icon_button(SharedString::from(format!("eco-bprize-x-{}", prize.id)), "x", p)
                                .tooltip(|window, cx| {
                                    crate::ui::overlay::Tip::new(t("serversettings.economy.gacha.removePrize"))
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let gone = gone.clone();
                                    this.change_banner(window, cx, |d| d.prizes.retain(|pr| pr.id != gone));
                                })),
                        ),
                );
            }
            // Add a prize: some currency, or any listing (gacha-only ones first).
            let open = self.economy.banner.adding.as_deref() == Some(tier.id.as_str());
            let full = draft.prizes.len() >= PRIZES_MAX;
            let toggle = tier.id.clone();
            group = group.child(
                div()
                    .id(SharedString::from(format!("eco-badd-{}", tier.id)))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(32.0))
                    .px(px(10.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .when(!full, |el| el.cursor_pointer().hover(|s| s.bg(p.secondary)))
                    .when(full, |el| el.opacity(0.5))
                    .child(icon(if open { "chevron-up" } else { "plus" }).size(px(14.0)))
                    .child(t("serversettings.economy.gacha.addPrize"))
                    .when(!full, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            let b = &mut this.economy.banner;
                            b.adding =
                                if b.adding.as_deref() == Some(toggle.as_str()) { None } else { Some(toggle.clone()) };
                            cx.notify();
                        }))
                    }),
            );
            if open && !full {
                let mut picks = div().flex().flex_wrap().gap(px(6.0));
                let add = |listing: Option<String>, tier_id: String| {
                    cx.listener(move |this: &mut Self, _, window, cx| {
                        this.economy.banner.made += 1;
                        let n = this.economy.banner.made;
                        this.economy.banner.adding = None;
                        let (listing, tier_id) = (listing.clone(), tier_id.clone());
                        this.change_banner(window, cx, |d| {
                            d.add_prize(&tier_id, listing.as_deref(), n);
                        });
                    })
                };
                picks = picks.child(
                    chip(
                        SharedString::from(format!("eco-badd-cur-{}", tier.id)),
                        &t("serversettings.economy.gacha.currencyPrize"),
                        false,
                        p,
                    )
                    .child(div().ml(px(6.0)).child(icon("coins").size(px(14.0))))
                    .on_click(add(None, tier.id.clone())),
                );
                for l in &listings {
                    let mut name = listing_name(l, roles, items);
                    if name.is_empty() {
                        name = t("chattools.economy.somethingFromShop");
                    }
                    if l.gacha_only {
                        name = format!("{name} · {}", t("chattools.economy.gacha.only"));
                    }
                    picks = picks.child(
                        chip(SharedString::from(format!("eco-badd-{}-{}", tier.id, l.id)), &name, false, p)
                            .on_click(add(Some(l.id.clone()), tier.id.clone())),
                    );
                }
                group = group.child(picks);
            }
            col = col.child(group);
        }
        col
    }
}
