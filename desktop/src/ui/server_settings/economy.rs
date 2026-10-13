//! The Economy page (Manage Economy, docs/economy.md): the server's
//! currencies, each with its name, icon (a standard emoji or one of the
//! server's), a line about it, whether members may send it, and how it's
//! earned (a rule per action: how much, a cooldown for messages and
//! reactions, a daily cap, a streak bonus for the daily reward); then the
//! shop's listings: a role (only harmless ones below you, with Manage
//! Roles), a profile item or anything else by name, each with its price,
//! stock, place and whether it's on sale. One thing is edited at a time,
//! in place, saved from the floating bar.

use super::pages::{boxed, focused, label};
use super::roles::switch;
use super::*;
use crate::core::economy::{
    self as eco, ACTIONS, CurrencyDraft, ListingDraft, amount_text, has_cooldown, listing_name,
};
use crate::pb::{CurrencyAction as Action, ShopListingKind as Kind};
use crate::ui::economy::{currency_icon, gold, money};
use crate::ui::theme::radius_xl;

/// What's open in the editor: a currency or a listing, new (no id) or not,
/// as it was and as it's being changed.
pub(super) enum Editor {
    Currency { id: Option<String>, before: CurrencyDraft, draft: CurrencyDraft },
    Listing { id: Option<String>, before: ListingDraft, draft: ListingDraft },
}

pub(super) struct Economy {
    editor: Option<Editor>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    emoji_query: Entity<InputState>,
    /// The game rule's games, one per line or separated by commas.
    games: Entity<TextareaState>,
    /// The icon's picker is open.
    icon_open: bool,
    /// The editor's number boxes, by what they hold ("amount-1", "price"...).
    numbers: HashMap<String, Entity<InputState>>,
    saving: bool,
    /// A currency or listing asked about before it's deleted, and one going.
    asking: Option<String>,
    deleting: Option<String>,
}

/// The number boxes the editors use.
fn number_keys() -> Vec<String> {
    let mut keys = Vec::new();
    for a in ACTIONS {
        let n = a as i32;
        keys.push(format!("amount-{n}"));
        keys.push(format!("cap-{n}"));
        if has_cooldown(a) {
            keys.push(format!("cooldown-{n}"));
        }
    }
    keys.extend(["bonus", "days", "price", "stock", "position"].map(str::to_owned));
    keys
}

impl Economy {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let typed = |this: &mut ServerSettingsView,
                     _: Entity<InputState>,
                     e: &InputEvent,
                     cx: &mut Context<ServerSettingsView>| {
            if matches!(e, InputEvent::Change) {
                this.economy_typed(cx);
            }
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.economy.currency.namePlaceholder")));
        let description =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.economy.currency.descriptionPlaceholder")));
        let emoji_query = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.economy.currency.findEmoji")));
        let games = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 6).placeholder(t("desktop.economy.currency.gamesPlaceholder"))
        });
        let mut subscriptions = vec![
            cx.subscribe(&name, typed),
            cx.subscribe(&games, |this: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.economy_typed(cx);
                }
            }),
            cx.subscribe(&description, typed),
            cx.subscribe(&emoji_query, |_: &mut ServerSettingsView, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut numbers = HashMap::new();
        for key in number_keys() {
            let state = cx.new(|cx| InputState::new(window, cx));
            subscriptions.push(cx.subscribe(&state, typed));
            numbers.insert(key, state);
        }
        let economy = Self {
            editor: None,
            name,
            description,
            emoji_query,
            games,
            icon_open: false,
            numbers,
            saving: false,
            asking: None,
            deleting: None,
        };
        (economy, subscriptions)
    }

    fn number(&self, key: &str) -> Option<&Entity<InputState>> {
        self.numbers.get(key)
    }
}

/// A box's number: what's typed, or 0 when it isn't one (the draft's checks then say so).
fn typed_number(text: &str) -> i64 {
    eco::parse_amount(text).unwrap_or(0)
}

impl ServerSettingsView {
    /// Something was typed in the editor: the draft follows its boxes.
    fn economy_typed(&mut self, cx: &mut Context<Self>) {
        let read: HashMap<String, String> = self
            .economy
            .numbers
            .iter()
            .map(|(k, v)| (k.clone(), v.read(cx).value().to_string()))
            .chain([
                ("name".to_owned(), self.economy.name.read(cx).value().to_string()),
                ("description".to_owned(), self.economy.description.read(cx).value().to_string()),
                ("games".to_owned(), self.economy.games.read(cx).value().to_string()),
            ])
            .collect();
        let get = |k: &str| read.get(k).map(String::as_str).unwrap_or_default();
        match &mut self.economy.editor {
            Some(Editor::Currency { draft, .. }) => {
                draft.name = get("name").to_owned();
                draft.description = get("description").to_owned();
                for r in &mut draft.rules {
                    let n = r.action as i32;
                    r.amount = typed_number(get(&format!("amount-{n}")));
                    r.daily_cap = typed_number(get(&format!("cap-{n}")));
                    if has_cooldown(r.action) {
                        r.cooldown_seconds =
                            typed_number(get(&format!("cooldown-{n}"))).clamp(-1, i64::from(i32::MAX)) as i32;
                    }
                    if r.action == Action::Daily {
                        r.streak_bonus = typed_number(get("bonus"));
                        r.streak_max_days = typed_number(get("days")).clamp(-1, i64::from(i32::MAX)) as i32;
                    }
                    if r.action == Action::GameMinute {
                        r.games = eco::parse_games(get("games"));
                    }
                }
            }
            Some(Editor::Listing { draft, .. }) => {
                draft.name = get("name").to_owned();
                draft.description = get("description").to_owned();
                draft.price = typed_number(get("price"));
                if draft.stock.is_some() {
                    draft.stock = Some(typed_number(get("stock")));
                }
                draft.position = typed_number(get("position")).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            }
            None => {}
        }
        self.error = None;
        cx.notify();
    }

    /// Fills the editor's boxes from what's opened in it.
    fn fill_economy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut values: Vec<(String, String)> = Vec::new();
        let mut games = String::new();
        let (name, description) = match &self.economy.editor {
            Some(Editor::Currency { draft, .. }) => {
                for r in &draft.rules {
                    let n = r.action as i32;
                    values.push((format!("amount-{n}"), r.amount.to_string()));
                    values.push((
                        format!("cap-{n}"),
                        if r.daily_cap > 0 { r.daily_cap.to_string() } else { String::new() },
                    ));
                    if has_cooldown(r.action) {
                        values.push((format!("cooldown-{n}"), r.cooldown_seconds.to_string()));
                    }
                    if r.action == Action::Daily {
                        values.push(("bonus".into(), r.streak_bonus.to_string()));
                        values.push(("days".into(), r.streak_max_days.to_string()));
                    }
                    if r.action == Action::GameMinute {
                        games = eco::games_text(&r.games);
                    }
                }
                (draft.name.clone(), draft.description.clone())
            }
            Some(Editor::Listing { draft, .. }) => {
                values.push(("price".into(), draft.price.to_string()));
                values.push(("stock".into(), draft.stock.unwrap_or(10).to_string()));
                values.push(("position".into(), draft.position.to_string()));
                (draft.name.clone(), draft.description.clone())
            }
            None => return,
        };
        for (key, value) in values {
            if let Some(state) = self.economy.number(&key).cloned() {
                state.update(cx, |s, cx| s.set_value(value, window, cx));
            }
        }
        self.economy.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.economy.description.update(cx, |s, cx| s.set_value(description, window, cx));
        self.economy.emoji_query.update(cx, |s, cx| s.set_value("", window, cx));
        self.economy.games.update(cx, |s, cx| s.set_value(games, window, cx));
    }

    fn edit_currency(&mut self, currency: Option<pb::Currency>, window: &mut Window, cx: &mut Context<Self>) {
        let before = match &currency {
            Some(c) => CurrencyDraft::of(c),
            None => CurrencyDraft::new(),
        };
        self.economy.editor =
            Some(Editor::Currency { id: currency.map(|c| c.id), before: before.clone(), draft: before });
        self.economy.icon_open = false;
        self.economy.asking = None;
        self.error = None;
        self.fill_economy(window, cx);
        self.economy.name.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn edit_listing(&mut self, listing: Option<pb::ShopListing>, window: &mut Window, cx: &mut Context<Self>) {
        let (currencies, count) = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.economy(&self.server))
                .map(|e| (e.currencies.clone(), e.listings.len()))
                .unwrap_or_default()
        });
        let before = match &listing {
            Some(l) => ListingDraft::of(l),
            None => {
                let first = currencies.first().map(|c| c.id.as_str()).unwrap_or_default();
                ListingDraft::new(first, Kind::Custom, count as i32)
            }
        };
        self.economy.editor =
            Some(Editor::Listing { id: listing.map(|l| l.id), before: before.clone(), draft: before });
        self.economy.asking = None;
        self.error = None;
        self.fill_economy(window, cx);
        cx.notify();
    }

    fn currency_draft(&mut self) -> Option<&mut CurrencyDraft> {
        match &mut self.economy.editor {
            Some(Editor::Currency { draft, .. }) => Some(draft),
            _ => None,
        }
    }

    fn listing_draft(&mut self) -> Option<&mut ListingDraft> {
        match &mut self.economy.editor {
            Some(Editor::Listing { draft, .. }) => Some(draft),
            _ => None,
        }
    }

    /// Saves what's open in the editor: makes it, or sends what changed.
    fn save_economy(&mut self, cx: &mut Context<Self>) {
        if self.economy.saving {
            return;
        }
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let count = self
            .core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.economy(&self.server)).map_or(0, |e| e.currencies.len()));
        let job: std::pin::Pin<Box<dyn Future<Output = Result<(), Problem>> + Send>> = match &self.economy.editor {
            Some(Editor::Currency { id, before, draft }) => {
                if let Some(problem) = draft.problem() {
                    self.error = Some(t(problem));
                    cx.notify();
                    return;
                }
                match id.clone() {
                    None => {
                        let change = draft.create(count as i32);
                        Box::pin(async move { core.create_currency(&key, &sid, change).await.map(|_| ()) })
                    }
                    Some(id) => {
                        let change = draft.change(before);
                        Box::pin(async move { core.update_currency(&key, &sid, &id, change).await.map(|_| ()) })
                    }
                }
            }
            Some(Editor::Listing { id, before, draft }) => {
                if let Some(problem) = draft.problem() {
                    self.error = Some(t(problem));
                    cx.notify();
                    return;
                }
                match id.clone() {
                    None => {
                        let new = draft.create();
                        Box::pin(async move { core.create_listing(&key, &sid, new).await.map(|_| ()) })
                    }
                    Some(id) => {
                        let change = draft.change(before);
                        Box::pin(async move { core.update_listing(&key, &sid, &id, change).await.map(|_| ()) })
                    }
                }
            }
            None => return,
        };
        self.economy.saving = true;
        self.error = None;
        self.run(cx, job, |this, result, cx| {
            this.economy.saving = false;
            match result {
                Ok(()) => {
                    this.economy.editor = None;
                    this.flash_saved(cx);
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn delete_economy_thing(&mut self, id: String, currency: bool, cx: &mut Context<Self>) {
        self.economy.asking = None;
        self.economy.deleting = Some(id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let gone = id.clone();
        self.run(
            cx,
            async move {
                if currency {
                    core.delete_currency(&key, &sid, &id).await
                } else {
                    core.delete_listing(&key, &sid, &id).await
                }
            },
            move |this, result, cx| {
                this.economy.deleting = None;
                match result {
                    Ok(()) => {
                        let open = match &this.economy.editor {
                            Some(Editor::Currency { id: Some(id), .. } | Editor::Listing { id: Some(id), .. }) => {
                                *id == gone
                            }
                            _ => false,
                        };
                        if open {
                            this.economy.editor = None;
                        }
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn economy_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (economy, emojis, roles, items, access) = self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return Default::default() };
            (
                i.economy(&self.server).cloned().unwrap_or_default(),
                i.emojis.get(&self.server).cloned().unwrap_or_default(),
                i.roles.get(&self.server).cloned().unwrap_or_default(),
                i.server_items.get(&self.server).cloned().unwrap_or_default(),
                i.access(&self.server),
            )
        });
        // The floating bar while something's open with changes (a new one always has some).
        let pending = match &self.economy.editor {
            Some(Editor::Currency { id, before, draft }) => Some(if id.is_none() { 1 } else { draft.changes(before) }),
            Some(Editor::Listing { id, before, draft }) => Some(if id.is_none() { 1 } else { draft.changes(before) }),
            None => None,
        };
        if let Some(n) = pending.filter(|n| *n > 0) {
            self.bar = Some(bar_with_error(
                "economy-bar",
                n,
                self.economy.saving,
                None,
                p,
                cx,
                |this, _, cx| {
                    this.economy.editor = None;
                    this.error = None;
                    cx.notify();
                },
                |this, _, cx| this.save_economy(cx),
            ));
        }

        let mut page = div().flex().flex_col().gap(px(28.0));
        // Currencies.
        let new_open = matches!(self.economy.editor, Some(Editor::Currency { id: None, .. }));
        let mut currencies = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(div().flex_1().child(super::pages::heading(
                    &t("desktop.economy.currency.title"),
                    Some(&t("desktop.economy.currency.about")),
                    p,
                )))
                .child(
                    super::pages::act(
                        "eco-new-currency",
                        t("desktop.economy.currency.new"),
                        icon("plus").size(px(16.0)),
                        Look::Primary,
                        p,
                    )
                    .when(new_open, |el| el.opacity(0.5))
                    .when(!new_open, |el| {
                        el.on_click(cx.listener(|this, _, window, cx| this.edit_currency(None, window, cx)))
                    }),
                ),
        );
        if new_open {
            currencies = currencies.child(self.currency_editor(None, &emojis, p, window, cx));
        }
        if economy.currencies.is_empty() && !new_open {
            currencies = currencies.child(empty_card("coins", &t("desktop.economy.currency.none"), p));
        }
        for (n, c) in economy.currencies.iter().enumerate() {
            let editing = matches!(&self.economy.editor, Some(Editor::Currency { id: Some(id), .. }) if *id == c.id);
            let card = if editing {
                self.currency_editor(Some(c), &emojis, p, window, cx)
            } else {
                self.currency_card(c, &emojis, p, window, cx)
            };
            currencies = currencies.child(motion::rise(
                div().child(card),
                SharedString::from(format!("eco-cur-in-{}", c.id)),
                Duration::from_millis((n.min(8) * 30) as u64),
                10.0,
            ));
        }
        page = page.child(currencies);

        // The shop.
        let new_listing = matches!(self.economy.editor, Some(Editor::Listing { id: None, .. }));
        let can_list = !economy.currencies.is_empty();
        let mut shop = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(div().flex_1().child(super::pages::heading(
                    &t("desktop.economy.listing.title"),
                    Some(&t("desktop.economy.listing.about")),
                    p,
                )))
                .child(
                    super::pages::act(
                        "eco-new-listing",
                        t("desktop.economy.listing.new"),
                        icon("plus").size(px(16.0)),
                        Look::Primary,
                        p,
                    )
                    .when(new_listing || !can_list, |el| el.opacity(0.5))
                    .when(!new_listing && can_list, |el| {
                        el.on_click(cx.listener(|this, _, window, cx| this.edit_listing(None, window, cx)))
                    }),
                ),
        );
        if new_listing {
            shop = shop.child(self.listing_editor(None, &economy, &emojis, &roles, &items, &access, p, window, cx));
        }
        if economy.listings.is_empty() && !new_listing {
            let text =
                if can_list { t("desktop.economy.listing.none") } else { t("desktop.economy.listing.needCurrency") };
            shop = shop.child(empty_card("store", &text, p));
        }
        for (n, l) in economy.listings.iter().enumerate() {
            let editing = matches!(&self.economy.editor, Some(Editor::Listing { id: Some(id), .. }) if *id == l.id);
            let card = if editing {
                self.listing_editor(Some(l), &economy, &emojis, &roles, &items, &access, p, window, cx)
            } else {
                self.listing_card(l, &economy, &emojis, &roles, &items, p, window, cx)
            };
            shop = shop.child(motion::rise(
                div().child(card),
                SharedString::from(format!("eco-l-in-{}", l.id)),
                Duration::from_millis((n.min(8) * 30) as u64),
                10.0,
            ));
        }
        page.child(shop).into_any_element()
    }

    /// Asked before deleting: what goes with it, Cancel and Delete.
    fn economy_asking(
        &self,
        id: &str,
        words: String,
        currency: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = id.to_owned();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(corner(12.0))
            .bg(alpha(p.destructive, 0.08))
            .child(div().flex_1().min_w(px(200.0)).text_sm().text_color(p.destructive).child(words))
            .child(soft_button("eco-ask-no", t("common.cancel"), p).on_click(cx.listener(|this, _, _, cx| {
                this.economy.asking = None;
                cx.notify();
            })))
            .child(
                danger_button("eco-ask-yes", t("desktop.economy.delete"), p)
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_economy_thing(id.clone(), currency, cx))),
            )
            .into_any_element()
    }

    fn currency_card(
        &self,
        c: &pb::Currency,
        emojis: &[pb::Emoji],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let gold = gold(p);
        let asking = self.economy.asking.as_deref() == Some(c.id.as_str());
        let deleting = self.economy.deleting.as_deref() == Some(c.id.as_str());
        let mut chips = div().flex().flex_wrap().gap(px(4.0));
        for r in &c.earn {
            let Ok(a) = Action::try_from(r.action) else { continue };
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(22.0))
                    .rounded_full()
                    .bg(alpha(p.muted, 0.8))
                    .text_size(px(11.0))
                    .text_color(p.muted_foreground)
                    .child(icon(eco::action_glyph(a)).size(px(12.0)))
                    .child(if a == Action::GameMinute {
                        format!("{} +{} · {}", t(eco::action_key(a)), amount_text(r.amount), eco::games_short(&r.games))
                    } else {
                        format!("{} +{}", t(eco::action_key(a)), amount_text(r.amount))
                    }),
            );
        }
        if c.transferable {
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(22.0))
                    .rounded_full()
                    .bg(alpha(p.primary, 0.12))
                    .text_size(px(11.0))
                    .text_color(p.primary)
                    .child(icon("send").size(px(12.0)))
                    .child(t("desktop.economy.currency.sendable")),
            );
        }
        let (edit, del) = (c.clone(), c.id.clone());
        let hover = alpha(p.primary, 0.3);
        div()
            .id(SharedString::from(format!("eco-cur-{}", c.id)))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .hover(move |s| s.border_color(hover))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .size(px(44.0))
                            .flex_none()
                            .rounded(corner(14.0))
                            .bg(gold.opacity(0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(currency_icon(c, emojis, 26.0)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::BOLD).truncate().child(c.name.clone()))
                            .when(!c.description.is_empty(), |el| {
                                el.child(div().text_sm().text_color(p.muted_foreground).child(c.description.clone()))
                            }),
                    )
                    .when(deleting, |el| el.child(spinner(format!("eco-del-{}", c.id), 16.0, window)))
                    .child(
                        soft_button(SharedString::from(format!("eco-edit-{}", c.id)), t("desktop.economy.edit"), p)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_currency(Some(edit.clone()), window, cx)
                            })),
                    )
                    .child(icon_button(SharedString::from(format!("eco-del-btn-{}", c.id)), "trash", p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.economy.asking = Some(del.clone());
                            cx.notify();
                        }),
                    )),
            )
            .when(!c.earn.is_empty() || c.transferable, |el| el.child(chips))
            .when(asking, |el| {
                el.child(self.economy_asking(
                    &c.id,
                    t_with("desktop.economy.currency.deleteAsk", &[("name", Arg::Str(&c.name))]),
                    true,
                    p,
                    cx,
                ))
            })
            .into_any_element()
    }

    /// A number box in the editor, `w` wide, with what it counts after it.
    fn economy_number(
        &self,
        key: &str,
        w: f32,
        unit: Option<String>,
        p: &Palette,
        window: &Window,
        cx: &gpui_kit::App,
    ) -> Div {
        let Some(state) = self.economy.number(key) else { return div() };
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(div().w(px(w)).child(boxed(
                Input::new(state).appearance(false),
                36.0,
                focused(state, window, cx),
                p,
            )))
            .when_some(unit, |el, u| el.child(div().text_sm().text_color(p.muted_foreground).child(u)))
    }

    fn currency_editor(
        &mut self,
        currency: Option<&pb::Currency>,
        emojis: &[pb::Emoji],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(Editor::Currency { draft, .. }) = &self.economy.editor else { return div().into_any_element() };
        let draft = draft.clone();
        let preview = pb::Currency {
            icon_emoji: draft.icon_emoji.clone(),
            icon_emoji_id: draft.icon_emoji_id.clone(),
            ..Default::default()
        };
        let gold = gold(p);
        let id_part = currency.map_or("new".to_owned(), |c| c.id.clone());
        let mut form = div().flex().flex_col().gap(px(16.0));
        // The icon, then the name.
        let icon_open = self.economy.icon_open;
        form = form.child(
            div()
                .flex()
                .items_end()
                .gap(px(12.0))
                .child(
                    div().flex().flex_col().gap(px(8.0)).child(label(&t("desktop.economy.currency.icon"))).child(
                        div()
                            .id("eco-icon-pick")
                            .size(px(40.0))
                            .rounded(radius_xl())
                            .border_1()
                            .border_color(if icon_open { alpha(p.primary, 0.6) } else { p.border.into() })
                            .bg(gold.opacity(0.1))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.scale(1.06))
                            .active(|s| s.scale(0.94))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.economy.icon_open = !this.economy.icon_open;
                                cx.notify();
                            }))
                            .child(currency_icon(&preview, emojis, 24.0)),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(label(&t("desktop.economy.currency.name")))
                        .child(boxed(
                            Input::new(&self.economy.name).appearance(false),
                            40.0,
                            focused(&self.economy.name, window, cx),
                            p,
                        )),
                ),
        );
        if icon_open {
            form = form.child(motion::rise(
                self.icon_picker(emojis, p, window, cx),
                SharedString::from("eco-icon-grid-in"),
                Duration::ZERO,
                6.0,
            ));
        }
        form = form
            .child(div().flex().flex_col().gap(px(8.0)).child(label(&t("desktop.economy.currency.description"))).child(
                boxed(
                    Input::new(&self.economy.description).appearance(false),
                    40.0,
                    focused(&self.economy.description, window, cx),
                    p,
                ),
            ))
            .child(
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
                                    .child(t("desktop.economy.currency.transferable")),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("desktop.economy.currency.transferableHint")),
                            ),
                    )
                    .child(switch(
                        SharedString::from(format!("eco-transfer-{id_part}")),
                        draft.transferable,
                        false,
                        cx,
                        |this: &mut Self, on, cx| {
                            if let Some(d) = this.currency_draft() {
                                d.transferable = on;
                            }
                            cx.notify();
                        },
                    )),
            );
        // How it's earned: a row per action, its numbers once it's on.
        let mut rules = div().flex().flex_col().gap(px(6.0)).child(super::pages::heading(
            &t("desktop.economy.currency.earning"),
            Some(&t("desktop.economy.currency.earningHint")),
            p,
        ));
        let games_offered =
            self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| eco::offered(Action::GameMinute, i)));
        for r in &draft.rules {
            let a = r.action;
            // An instance without the game rule never sees it offered (one already set still shows).
            if a == Action::GameMinute && !games_offered && !r.enabled {
                continue;
            }
            let n = a as i32;
            let mut row = div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(corner(12.0))
                .bg(if r.enabled { alpha(p.primary, 0.05) } else { alpha(p.muted, 0.4) })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .size(px(30.0))
                                .flex_none()
                                .rounded(corner(10.0))
                                .bg(if r.enabled { gold.opacity(0.15) } else { alpha(p.muted, 0.8) })
                                .text_color(if r.enabled { gold } else { p.muted_foreground.into() })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(eco::action_glyph(a)).size(px(16.0))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().font_weight(FontWeight::BOLD).child(t(eco::action_key(a))))
                                .child(
                                    div().text_xs().text_color(p.muted_foreground).child(t(eco::action_hint_key(a))),
                                ),
                        )
                        .child(switch(
                            SharedString::from(format!("eco-rule-{id_part}-{n}")),
                            r.enabled,
                            false,
                            cx,
                            move |this: &mut Self, on, cx| {
                                if let Some(d) = this.currency_draft()
                                    && let Some(r) = d.rules.iter_mut().find(|r| r.action == a)
                                {
                                    r.enabled = on;
                                }
                                cx.notify();
                            },
                        )),
                );
            if r.enabled {
                let mut numbers = div()
                    .flex()
                    .flex_wrap()
                    .gap(px(16.0))
                    .child(rule_field(
                        t("desktop.economy.currency.amount"),
                        self.economy_number(&format!("amount-{n}"), 110.0, None, p, window, cx),
                    ))
                    .child(rule_field(
                        t("desktop.economy.currency.dailyCap"),
                        self.economy_number(
                            &format!("cap-{n}"),
                            110.0,
                            Some(t("desktop.economy.currency.noCap")),
                            p,
                            window,
                            cx,
                        ),
                    ));
                if has_cooldown(a) {
                    numbers = numbers.child(rule_field(
                        t("desktop.economy.currency.cooldown"),
                        self.economy_number(
                            &format!("cooldown-{n}"),
                            90.0,
                            Some(t("desktop.economy.currency.seconds")),
                            p,
                            window,
                            cx,
                        ),
                    ));
                }
                if a == Action::Daily {
                    numbers = numbers
                        .child(rule_field(
                            t("desktop.economy.currency.streakBonus"),
                            self.economy_number("bonus", 90.0, None, p, window, cx),
                        ))
                        .child(rule_field(
                            t("desktop.economy.currency.streakDays"),
                            self.economy_number("days", 70.0, Some(t("desktop.economy.currency.days")), p, window, cx),
                        ));
                }
                if a == Action::GameMinute {
                    let games = self.economy.games.clone();
                    numbers = numbers.child(
                        div().w_full().child(rule_field(
                            t("desktop.economy.currency.games"),
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(crate::ui::instance_home::focus_ring(
                                    div()
                                        .w_full()
                                        .min_h(px(62.0))
                                        .px(px(2.0))
                                        .rounded(radius_xl())
                                        .border_1()
                                        .border_color(p.border)
                                        .text_sm()
                                        .child(Textarea::new(&games).appearance(false)),
                                    focused(&games, window, cx),
                                    p,
                                ))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(p.muted_foreground)
                                        .child(t("desktop.economy.currency.gamesHint")),
                                ),
                        )),
                    );
                }
                row = row.child(motion::rise(
                    numbers,
                    SharedString::from(format!("eco-rule-n-{n}")),
                    Duration::ZERO,
                    -6.0,
                ));
            }
            rules = rules.child(row);
        }
        form = form.child(rules);
        if let Some(problem) = draft.problem() {
            form = form.child(div().text_sm().text_color(p.muted_foreground).child(t(problem)));
        }
        editor_card(
            if currency.is_some() { t("desktop.economy.currency.editing") } else { t("desktop.economy.currency.new") },
            form,
            p,
        )
    }

    /// The icon's choices: the server's emoji, then standard ones (found by name when searched).
    fn icon_picker(&mut self, emojis: &[pb::Emoji], p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let query = self.economy.emoji_query.read(cx).value().trim().to_lowercase();
        let mut grid = div().flex().flex_wrap();
        // (characters, custom emoji id, picture)
        let mut cells: Vec<(String, String, Option<String>)> = emojis
            .iter()
            .filter(|e| query.is_empty() || e.name.to_lowercase().contains(&query))
            .map(|e| (String::new(), e.id.clone(), Some(e.url.clone())))
            .collect();
        cells.extend(
            crate::ui::emoji::standard()
                .iter()
                .flat_map(|g| &g.emojis)
                .filter(|e| {
                    query.is_empty()
                        || e.names.iter().any(|n| n.contains(&query))
                        || e.words.to_lowercase().contains(&query)
                })
                .take(if query.is_empty() { 64 } else { 96 })
                .map(|e| (e.char.clone(), String::new(), None)),
        );
        for (n, (chars, custom, url)) in cells.into_iter().enumerate() {
            let face = match url {
                Some(url) => crate::ui::widgets::picture(url).size(px(22.0)).into_any_element(),
                None => div().text_size(px(19.0)).child(chars.clone()).into_any_element(),
            };
            let hover = alpha(p.primary, 0.12);
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("eco-e-{n}")))
                    .size(px(34.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .active(|s| s.scale(0.9))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.currency_draft() {
                            // A custom emoji keeps no characters; a standard one no id.
                            d.icon_emoji = chars.clone();
                            d.icon_emoji_id = custom.clone();
                        }
                        this.economy.icon_open = false;
                        cx.notify();
                    }))
                    .child(face),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(8.0))
            .rounded(corner(12.0))
            .bg(p.secondary)
            .child(boxed(
                Input::new(&self.economy.emoji_query).appearance(false),
                36.0,
                focused(&self.economy.emoji_query, window, cx),
                p,
            ))
            .child(crate::ui::widgets::inner_scroll("eco-icon-grid", window, cx).max_h(px(170.0)).child(grid))
    }

    #[allow(clippy::too_many_arguments)]
    fn listing_card(
        &self,
        l: &pb::ShopListing,
        economy: &eco::Economy,
        emojis: &[pb::Emoji],
        roles: &[pb::Role],
        items: &[pb::ProfileItem],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = Kind::try_from(l.kind).unwrap_or(Kind::Custom);
        let currency = economy.currency(&l.currency_id).cloned().unwrap_or_default();
        let name = listing_name(l, roles, items);
        let asking = self.economy.asking.as_deref() == Some(l.id.as_str());
        let deleting = self.economy.deleting.as_deref() == Some(l.id.as_str());
        let (glyph, tint): (&str, Hsla) = match kind {
            Kind::Role => (
                "shield",
                roles
                    .iter()
                    .find(|r| r.id == l.target_id)
                    .and_then(|r| r.color)
                    .map(|c| gpui_kit::rgb(c as u32).into())
                    .unwrap_or(p.muted_foreground.into()),
            ),
            Kind::ProfileItem => ("sparkles", p.primary.into()),
            _ => ("package", gold(p)),
        };
        let mut facts = div().flex().flex_wrap().items_center().gap(px(10.0)).text_xs().text_color(p.muted_foreground);
        facts = facts.child(t_with("desktop.economy.listing.sold", &[("count", Arg::Num(l.sold))]));
        facts = facts.child(match l.stock {
            Some(n) => t_with("desktop.economy.shop.left", &[("count", Arg::Num(n))]),
            None => t("desktop.economy.listing.unlimited"),
        });
        if !l.available {
            facts = facts.child(
                div()
                    .px(px(6.0))
                    .rounded_full()
                    .bg(alpha(p.muted, 0.9))
                    .font_weight(FontWeight::BOLD)
                    .child(t("desktop.economy.listing.hidden")),
            );
        }
        let (edit, del) = (l.clone(), l.id.clone());
        let hover = alpha(p.primary, 0.3);
        div()
            .id(SharedString::from(format!("eco-l-{}", l.id)))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .hover(move |s| s.border_color(hover))
            .when(!l.available, |el| el.opacity(0.75))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .size(px(40.0))
                            .flex_none()
                            .rounded(corner(12.0))
                            .bg(tint.opacity(0.14))
                            .text_color(tint)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(glyph).size(px(20.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(div().font_weight(FontWeight::BOLD).truncate().child(name))
                                    .child(div().text_sm().child(money(&currency, emojis, l.price, 15.0))),
                            )
                            .child(facts),
                    )
                    .when(deleting, |el| el.child(spinner(format!("eco-ldel-{}", l.id), 16.0, window)))
                    .child(
                        soft_button(SharedString::from(format!("eco-ledit-{}", l.id)), t("desktop.economy.edit"), p)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_listing(Some(edit.clone()), window, cx)
                            })),
                    )
                    .child(icon_button(SharedString::from(format!("eco-ldel-btn-{}", l.id)), "trash", p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.economy.asking = Some(del.clone());
                            cx.notify();
                        }),
                    )),
            )
            .when(asking, |el| {
                el.child(self.economy_asking(&l.id, t("desktop.economy.listing.deleteAsk"), false, p, cx))
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn listing_editor(
        &mut self,
        listing: Option<&pb::ShopListing>,
        economy: &eco::Economy,
        emojis: &[pb::Emoji],
        roles: &[pb::Role],
        items: &[pb::ProfileItem],
        access: &crate::core::permissions::Access,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(Editor::Listing { draft, .. }) = &self.economy.editor else { return div().into_any_element() };
        let draft = draft.clone();
        let new = listing.is_none();
        let mut form = div().flex().flex_col().gap(px(16.0));
        if new {
            // What it sells.
            let mut kinds = div().flex().flex_wrap().gap(px(6.0));
            for (kind, key, glyph) in [
                (Kind::Role, "desktop.economy.kind.role", "shield"),
                (Kind::ProfileItem, "desktop.economy.kind.item", "sparkles"),
                (Kind::Custom, "desktop.economy.kind.custom", "package"),
            ] {
                let on = draft.kind == kind;
                kinds = kinds.child(
                    chip(SharedString::from(format!("eco-kind-{}", kind as i32)), &t(key), on, p)
                        .child(div().ml(px(6.0)).child(icon(glyph).size(px(14.0))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.listing_draft() {
                                d.kind = kind;
                                d.target_id.clear();
                            }
                            cx.notify();
                        })),
                );
            }
            form = form.child(
                div().flex().flex_col().gap(px(8.0)).child(label(&t("desktop.economy.listing.kind"))).child(kinds),
            );
            // In which currency.
            let mut picks = div().flex().flex_wrap().gap(px(6.0));
            for c in &economy.currencies {
                let on = draft.currency_id == c.id;
                let id = c.id.clone();
                picks = picks.child(
                    chip(SharedString::from(format!("eco-lcur-{}", c.id)), &c.name, on, p)
                        .child(div().ml(px(6.0)).child(currency_icon(c, emojis, 14.0)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.listing_draft() {
                                d.currency_id = id.clone();
                            }
                            cx.notify();
                        })),
                );
            }
            form = form.child(
                div().flex().flex_col().gap(px(8.0)).child(label(&t("desktop.economy.give.currency"))).child(picks),
            );
            // Which role or item.
            match draft.kind {
                Kind::Role => {
                    let mut sellable: Vec<&pb::Role> =
                        roles.iter().filter(|r| eco::role_sellable(r, &self.server, access)).collect();
                    sellable.sort_by_key(|r| std::cmp::Reverse(r.position));
                    let mut chips = div().flex().flex_wrap().gap(px(6.0));
                    for r in &sellable {
                        let on = draft.target_id == r.id;
                        let id = r.id.clone();
                        let dot: Hsla =
                            r.color.map(|c| gpui_kit::rgb(c as u32).into()).unwrap_or(p.muted_foreground.into());
                        chips = chips.child(
                            chip(SharedString::from(format!("eco-lrole-{}", r.id)), &r.name, on, p)
                                .child(div().ml(px(6.0)).size(px(8.0)).rounded_full().bg(dot))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.listing_draft() {
                                        d.target_id = id.clone();
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    form = form.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(label(&t("desktop.economy.kind.role")))
                            .child(if sellable.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("desktop.economy.listing.noRoles"))
                                    .into_any_element()
                            } else {
                                chips.into_any_element()
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(t("desktop.economy.listing.roleHint")),
                            ),
                    );
                }
                Kind::ProfileItem => {
                    let mut chips = div().flex().flex_wrap().gap(px(6.0));
                    for item in items {
                        let on = draft.target_id == item.id;
                        let id = item.id.clone();
                        let glyph = if item.kind == pb::ProfileItemKind::Decoration as i32 {
                            "circle-user-round"
                        } else {
                            "sparkles"
                        };
                        chips = chips.child(
                            chip(SharedString::from(format!("eco-litem-{}", item.id)), &item.name, on, p)
                                .child(div().ml(px(6.0)).child(icon(glyph).size(px(14.0))))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = this.listing_draft() {
                                        d.target_id = id.clone();
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    form = form.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(label(&t("desktop.economy.kind.item")))
                            .child(if items.is_empty() {
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("desktop.economy.listing.noItems"))
                                    .into_any_element()
                            } else {
                                chips.into_any_element()
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(t("desktop.economy.listing.itemHint")),
                            ),
                    );
                }
                _ => {}
            }
        }
        let name_label = if draft.kind == Kind::Custom {
            t("desktop.economy.listing.name")
        } else {
            t("desktop.economy.listing.nameOptional")
        };
        form = form
            .child(div().flex().flex_col().gap(px(8.0)).child(label(&name_label)).child(boxed(
                Input::new(&self.economy.name).appearance(false),
                40.0,
                focused(&self.economy.name, window, cx),
                p,
            )))
            .child(
                div().flex().flex_col().gap(px(8.0)).child(label(&t("desktop.economy.currency.description"))).child(
                    boxed(
                        Input::new(&self.economy.description).appearance(false),
                        40.0,
                        focused(&self.economy.description, window, cx),
                        p,
                    ),
                ),
            );
        let currency = economy.currency(&draft.currency_id).cloned().unwrap_or_default();
        let limited = draft.stock.is_some();
        form = form.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(20.0))
                .child(rule_field(
                    t("desktop.economy.listing.price"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(currency_icon(&currency, emojis, 18.0))
                        .child(self.economy_number("price", 130.0, None, p, window, cx)),
                ))
                .child(rule_field(
                    t("desktop.economy.listing.stock"),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(switch(
                            SharedString::from("eco-limited"),
                            limited,
                            false,
                            cx,
                            |this: &mut Self, on, cx| {
                                let typed = this.economy.number("stock").map(|s| typed_number(&s.read(cx).value()));
                                if let Some(d) = this.listing_draft() {
                                    d.stock = on.then(|| typed.unwrap_or(0));
                                }
                                cx.notify();
                            },
                        ))
                        .child(if limited {
                            self.economy_number("stock", 90.0, Some(t("desktop.economy.listing.left")), p, window, cx)
                                .into_any_element()
                        } else {
                            div()
                                .text_sm()
                                .text_color(p.muted_foreground)
                                .child(t("desktop.economy.listing.unlimited"))
                                .into_any_element()
                        }),
                ))
                .child(rule_field(
                    t("desktop.economy.listing.position"),
                    self.economy_number("position", 70.0, None, p, window, cx),
                )),
        );
        form = form.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .child(
                            div().text_sm().font_weight(FontWeight::BOLD).child(t("desktop.economy.listing.available")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.muted_foreground)
                                .child(t("desktop.economy.listing.availableHint")),
                        ),
                )
                .child(switch(
                    SharedString::from("eco-available"),
                    draft.available,
                    false,
                    cx,
                    |this: &mut Self, on, cx| {
                        if let Some(d) = this.listing_draft() {
                            d.available = on;
                        }
                        cx.notify();
                    },
                )),
        );
        if let Some(problem) = draft.problem() {
            form = form.child(div().text_sm().text_color(p.muted_foreground).child(t(problem)));
        }
        editor_card(if new { t("desktop.economy.listing.new") } else { t("desktop.economy.listing.editing") }, form, p)
    }
}

/// A rule's or listing's number with its name over it.
fn rule_field(name: String, body: impl IntoElement) -> Div {
    div().flex().flex_col().gap(px(6.0)).child(div().text_xs().font_weight(FontWeight::BOLD).child(name)).child(body)
}

/// The editor's frame: ringed in the primary color, with what it's editing over it.
fn editor_card(title: String, form: Div, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .p(px(16.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(alpha(p.primary, 0.45))
            .bg(alpha(p.background, 0.6))
            .shadow_lg()
            .child(
                div().text_xs().font_weight(FontWeight::EXTRA_BOLD).text_color(p.primary).child(title.to_uppercase()),
            )
            .child(form),
        SharedString::from(format!("eco-editor-{title}")),
        Duration::ZERO,
        8.0,
    )
    .into_any_element()
}

/// Nothing here yet: an icon and a line, in a dashed box.
fn empty_card(glyph: &'static str, text: &str, p: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .py(px(28.0))
        .rounded(corner(16.0))
        .border_1()
        .border_dashed()
        .border_color(p.border)
        .text_sm()
        .text_color(p.muted_foreground)
        .child(motion::pop(
            div().child(icon(glyph).size(px(28.0))),
            SharedString::from(format!("eco-empty-{glyph}")),
            0.5,
            -12.0,
            Duration::from_millis(60),
        ))
        .child(text.to_owned())
        .into_any_element()
}
