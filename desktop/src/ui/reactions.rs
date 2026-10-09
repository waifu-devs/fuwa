//! Reactions under a message, as the web app draws them: a chip per emoji
//! with its count (lit when one of them is yours) that adds or takes off
//! your reaction, who reacted when pointed at, and a button at the end that
//! opens the emoji picker by the pointer. Chips spring in and out and their
//! counts roll; with reduced motion they just change.
//!
//! In a server's channels the instance counts them (`core/reactions.rs`);
//! in direct messages and secure channels this device tallies them and knows
//! who reacted without asking.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, RenderOnce, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::reactions::EmojiKey;
use crate::core::store::InstanceState;
use crate::core::vault::ReactionMark;
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::emoji::{Catalog, Choice, InColor as _};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{icon, pal};

/// What a right-click menu offers first, before you've reacted with much.
const QUICK: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🎉"];
/// Names shown before "and N more".
const NAMES: usize = 3;

/// One emoji's chip.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub key: EmojiKey,
    /// A server emoji's name.
    pub name: String,
    pub animated: bool,
    /// A server emoji's picture.
    pub url: Option<String>,
    pub count: u32,
    pub me: bool,
    /// In an encrypted place: who has it on, by name, the earliest first.
    /// None where the instance is asked.
    pub who: Option<Vec<String>>,
}

impl Chip {
    /// How it reads in words: the emoji itself, or `:name:`.
    pub fn label(&self) -> String {
        if self.key.emoji_id.is_empty() { self.key.emoji.clone() } else { format!(":{}:", self.name) }
    }

    pub fn reaction(&self) -> pb::Reaction {
        pb::Reaction {
            emoji: self.key.emoji.clone(),
            emoji_id: self.key.emoji_id.clone(),
            emoji_name: self.name.clone(),
            animated: self.animated,
            count: self.count,
            me: self.me,
        }
    }
}

/// A message's reactions as its row draws them, and what you may do with them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReactBits {
    pub chips: Vec<Chip>,
    /// You may react here (taking yours off needs nothing).
    pub can_add: bool,
    /// You may take others' off (Manage Messages).
    pub can_clear: bool,
    /// Only standard emoji: a conversation or a secure channel.
    pub standard_only: bool,
}

impl ReactBits {
    pub fn digest(&self, h: &mut impl Hasher) {
        for c in &self.chips {
            (c.key.id(), c.count, c.me, &c.url, &c.who).hash(h);
        }
        (self.can_add, self.can_clear, self.standard_only).hash(h);
    }

    /// A server message's reactions; a custom emoji the server no longer has isn't shown.
    pub fn server(i: &InstanceState, server: &str, m: &pb::Message, can_add: bool, can_clear: bool) -> Self {
        let emojis = i.emojis.get(server);
        let chips = m
            .reactions
            .iter()
            .filter(|r| r.count > 0)
            .filter_map(|r| {
                let url = if r.emoji_id.is_empty() {
                    None
                } else {
                    Some(emojis?.iter().find(|e| e.id == r.emoji_id)?.url.clone())
                };
                Some(Chip {
                    key: EmojiKey::of(r),
                    name: r.emoji_name.clone(),
                    animated: r.animated,
                    url,
                    count: r.count,
                    me: r.me,
                    who: None,
                })
            })
            .collect();
        Self { chips, can_add, can_clear, standard_only: false }
    }

    /// An encrypted message's reactions, from what this device tallied.
    pub fn encrypted(marks: &[ReactionMark], me: &str, can_add: bool, name_of: &dyn Fn(&str) -> String) -> Self {
        let chips = crate::core::reactions::tally(marks, me)
            .into_iter()
            .map(|r| {
                let who = crate::core::reactions::reactors(marks, &r.emoji)
                    .iter()
                    .map(|u| if u == me { t("chattools.reactions.you") } else { name_of(u) })
                    .collect();
                Chip {
                    key: EmojiKey::of(&r),
                    name: String::new(),
                    animated: false,
                    url: None,
                    count: r.count,
                    me: r.me,
                    who: Some(who),
                }
            })
            .collect();
        Self { chips, can_add, can_clear: false, standard_only: true }
    }
}

/// The people in a tooltip: the first few names, then "and N more" for the rest of `count`.
fn people(names: &[String], count: u32) -> String {
    let shown: Vec<&str> = names.iter().take(NAMES).map(String::as_str).collect();
    let rest = (count as usize).max(names.len()).saturating_sub(shown.len());
    let list = shown.join(", ");
    if rest == 0 {
        list
    } else {
        format!("{list} {}", t_with("chattools.reactions.andMore", &[("count", Arg::Num(rest as i64))]))
    }
}

/// "Ann, You and 2 more reacted with 👍", with you first when you're one of them.
pub fn tip_text(chip: &Chip, names: &[String]) -> String {
    let you = t("chattools.reactions.you");
    let mut names = names.to_vec();
    if let Some(at) = names.iter().position(|n| *n == you) {
        let mine = names.remove(at);
        names.insert(0, mine);
    }
    if names.is_empty() {
        return t_with("chattools.reactions.who", &[("emoji", Arg::Str(&chip.label()))]);
    }
    t_with(
        "chattools.reactions.tooltip",
        &[("people", Arg::Str(&people(&names, chip.count))), ("emoji", Arg::Str(&chip.label()))],
    )
}

/// Who reacted with an emoji, as last asked of the instance.
#[derive(Debug, Clone)]
pub struct SeenReactors {
    /// The count it was asked at: a new count asks again.
    pub count: u32,
    /// Their names ("You" for you), the earliest first; None while asking.
    pub names: Option<Vec<String>>,
    pub failed: bool,
}

/// The picker opened to react: to which message, by what point, and whether only standard emoji go.
#[derive(Debug, Clone)]
pub struct ReactAt {
    pub msg: String,
    pub at: Point<Pixels>,
    pub standard_only: bool,
}

// ───────────────────────── The row ─────────────────────────

/// What a row of chips remembers between frames: the chips it showed, when
/// each new one came (so it pops in once), and the ones going.
struct Seen {
    last: Vec<Chip>,
    born: HashMap<String, u64>,
    gone: Vec<(usize, Chip, Instant)>,
    next: u64,
}

impl Seen {
    fn new(chips: &[Chip]) -> Self {
        Self { last: chips.to_vec(), born: HashMap::new(), gone: Vec::new(), next: 0 }
    }

    /// Notes what changed since the last frame.
    fn step(&mut self, chips: &[Chip], still: bool) {
        let ids: Vec<String> = chips.iter().map(|c| c.key.id()).collect();
        for id in &ids {
            if !self.last.iter().any(|c| c.key.id() == *id) {
                self.next += 1;
                self.born.insert(id.clone(), self.next);
            }
        }
        for (n, c) in self.last.iter().enumerate() {
            if !ids.contains(&c.key.id()) && !still {
                self.gone.push((n, c.clone(), Instant::now()));
            }
        }
        self.born.retain(|id, _| ids.contains(id));
        self.gone.retain(|(_, c, at)| !ids.contains(&c.key.id()) && at.elapsed() < motion::LEAVE && !still);
        self.last = chips.to_vec();
    }
}

/// A message's chips, and the button that adds one.
#[derive(IntoElement)]
pub(crate) struct ReactionRow {
    pub msg: String,
    pub bits: Rc<ReactBits>,
    pub this: WeakEntity<FuwaApp>,
    pub key: String,
}

impl RenderOnce for ReactionRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let p = pal(cx);
        let still = cx.reduce_motion();
        let chips = self.bits.chips.clone();
        let seen =
            window.use_keyed_state(SharedString::from(format!("reacts|{}", self.msg)), cx, |_, _| Seen::new(&chips));
        let (born, gone) = seen.update(cx, |s, _| {
            s.step(&chips, still);
            (s.born.clone(), s.gone.clone())
        });
        if !gone.is_empty() {
            window.request_animation_frame();
        }
        let mut drawn: Vec<AnyElement> = chips
            .iter()
            .map(|c| {
                let el = chip(&self, c, &p);
                match born.get(&c.key.id()) {
                    // A new one pops in, once.
                    Some(n) if !still => motion::pop(
                        div().child(el),
                        SharedString::from(format!("react-in|{}|{}|{n}", self.msg, c.key.id())),
                        0.4,
                        0.0,
                        Duration::ZERO,
                    )
                    .into_any_element(),
                    _ => el,
                }
            })
            .collect();
        // Ones going shrink and fade where they were.
        for (at, c, since) in gone {
            let t = gpui_kit::ease_out_quint()((since.elapsed().as_secs_f32() / motion::LEAVE.as_secs_f32()).min(1.0));
            let el = div().opacity(1.0 - t).scale(1.0 - 0.4 * t).child(glyph_and_count(&self.msg, &c, &p, false));
            let el = shell(div(), &c, &p).child(el).into_any_element();
            let at = at.min(drawn.len());
            drawn.insert(at, el);
        }
        if drawn.is_empty() {
            return div();
        }
        let add = (self.bits.can_add && !chips.is_empty()).then(|| add_button(&self, &p));
        div().flex().flex_wrap().items_center().gap(px(4.0)).mt(px(4.0)).children(drawn).children(add)
    }
}

/// A chip's outline: lit when one of them is yours.
fn shell<E: gpui_kit::Styled>(el: E, c: &Chip, p: &Palette) -> E {
    let (bg, border): (Hsla, Hsla) = if c.me {
        (alpha(p.primary, 0.15), alpha(p.primary, 0.55))
    } else {
        (alpha(p.muted, 0.85), alpha(p.border, 0.0))
    };
    el.h(px(26.0))
        .px(px(7.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(corner(9.0))
        .border_1()
        .border_color(border)
        .bg(bg)
}

/// The emoji, then its count, rolling as it changes.
fn glyph_and_count(msg: &str, c: &Chip, p: &Palette, rolling: bool) -> gpui_kit::Div {
    let fg: Rgba = if c.me { p.primary } else { p.muted_foreground };
    let id = format!("react-n|{msg}|{}", c.key.id());
    div().flex().items_center().gap(px(5.0)).child(glyph(c, 16.0)).child(
        div().text_size(px(12.5)).line_height(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(fg).map(|el| {
            if rolling {
                el.child(motion::rolling(id, u64::from(c.count), None, 12.5))
            } else {
                el.child(crate::core::i18n::number(i64::from(c.count)))
            }
        }),
    )
}

/// An emoji as a reaction draws it: a server's own picture, or the character.
fn glyph(c: &Chip, size: f32) -> AnyElement {
    let base = div().size(px(size + 2.0)).flex_none().flex().items_center().justify_center();
    match &c.url {
        Some(url) => base
            .child({
                use gpui_kit::StyledImage as _;
                gpui_kit::img(SharedString::from(url.clone())).size(px(size)).object_fit(gpui_kit::ObjectFit::Contain)
            })
            .into_any_element(),
        None => base.in_color().text_size(px(size * 0.9)).child(c.key.emoji.clone()).into_any_element(),
    }
}

fn chip(row: &ReactionRow, c: &Chip, p: &Palette) -> AnyElement {
    let msg = row.msg.clone();
    // Yours can always come off; adding needs Add Reactions.
    let pressable = c.me || row.bits.can_add;
    let hover_border = alpha(p.primary, if c.me { 0.8 } else { 0.35 });
    let el = shell(div().id(SharedString::from(format!("react|{msg}|{}", c.key.id()))), c, p)
        .child(glyph_and_count(&msg, c, p, true))
        .when(pressable, |el| {
            let (this, msg, r, on) = (row.this.clone(), msg.clone(), c.reaction(), !c.me);
            el.cursor_pointer().hover(move |s| s.border_color(hover_border)).active(|s| s.scale(0.92)).on_click(
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.react_to(msg.clone(), r.clone(), on, cx));
                },
            )
        });
    let tip_chip = c.clone();
    match &c.who {
        Some(names) => {
            let text = tip_text(c, names);
            el.tooltip(move |window, cx| crate::ui::overlay::Tip::new(text.clone()).build(window, cx))
                .into_any_element()
        }
        None => {
            let (this, hover_this) = (row.this.clone(), row.this.clone());
            let cache = reactors_key(&row.key, &msg, &c.key);
            let hover_chip = c.clone();
            el.on_hover(move |hovered: &bool, _, cx| {
                if *hovered {
                    let _ = hover_this.update(cx, |this, cx| this.load_reactors(msg.clone(), &hover_chip, cx));
                }
            })
            .tooltip(move |_, cx| ReactorsTip::build(this.clone(), cache.clone(), tip_chip.clone(), cx))
            .into_any_element()
        }
    }
}

/// The "+" at the end that opens the picker by itself.
fn add_button(row: &ReactionRow, p: &Palette) -> AnyElement {
    let (this, msg, standard_only) = (row.this.clone(), row.msg.clone(), row.bits.standard_only);
    let fg = p.foreground;
    let border = alpha(p.primary, 0.35);
    div()
        .id(SharedString::from(format!("react-add|{}", row.msg)))
        .h(px(26.0))
        .w(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(corner(9.0))
        .border_1()
        .border_color(alpha(p.border, 0.0))
        .bg(alpha(p.muted, 0.5))
        .text_color(p.muted_foreground)
        .cursor_pointer()
        .hover(move |s| s.text_color(fg).border_color(border))
        .active(|s| s.scale(0.9))
        .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chattools.reactions.add")).build(window, cx))
        .on_click(move |ev, window, cx| {
            let at = ev.position();
            let _ = this.update(cx, |this, cx| this.open_react_picker(msg.clone(), standard_only, at, window, cx));
        })
        .child(icon("face-slightly-smiling-plus").size(px(16.0)))
        .into_any_element()
}

// ───────────────────────── Who reacted ─────────────────────────

/// A chip's tooltip in a server: who reacted, read from the instance when
/// first pointed at and drawn again as the answer comes.
struct ReactorsTip {
    app: WeakEntity<FuwaApp>,
    cache: String,
    chip: Chip,
    bg: Rgba,
    fg: Rgba,
}

impl ReactorsTip {
    fn build(app: WeakEntity<FuwaApp>, cache: String, chip: Chip, cx: &mut App) -> gpui_kit::AnyView {
        let p = pal(cx);
        cx.new(|cx| {
            if let Some(a) = app.upgrade() {
                cx.observe(&a, |_, _, cx| cx.notify()).detach();
            }
            ReactorsTip { app, cache, chip, bg: p.primary, fg: p.primary_foreground }
        })
        .into()
    }
}

impl gpui_kit::Render for ReactorsTip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let seen = self.app.upgrade().and_then(|a| a.read(cx).reactors.get(&self.cache).cloned());
        let text = match seen {
            Some(SeenReactors { names: Some(names), failed: false, .. }) => tip_text(&self.chip, &names),
            Some(SeenReactors { failed: true, .. }) => t("chattools.reactions.failed"),
            _ => t("chattools.reactions.loading"),
        };
        let tip = div()
            .font_family(crate::ui::theme::FONT)
            .bg(self.bg)
            .text_color(self.fg)
            .rounded(crate::ui::theme::radius_md())
            .px(px(12.0))
            .py(px(6.0))
            .max_w(px(280.0))
            .text_xs()
            .line_height(px(16.0))
            .child(text);
        div().p(px(12.0)).child(motion::spring_in(tip, "tip-in", (300.0, 25.0), Duration::ZERO, |el, t| {
            el.opacity(t.clamp(0.0, 1.0)).transform_origin(0.5, 0.0).scale(0.5 + 0.5 * t)
        }))
    }
}

/// Whether two standard emoji are the same but for variation selectors.
fn same_emoji(a: &str, b: &str) -> bool {
    let bare = |s: &str| s.chars().filter(|c| *c != '\u{fe0f}' && *c != '\u{fe0e}').collect::<String>();
    a == b || bare(a) == bare(b)
}

/// Where who reacted with an emoji is kept.
fn reactors_key(key: &str, msg: &str, emoji: &EmojiKey) -> String {
    format!("{key}|{msg}|{}", emoji.id())
}

// ───────────────────────── Doing it ─────────────────────────

impl FuwaApp {
    /// What a reaction from the picker or a menu is: a standard emoji (as
    /// toned), or one of the open server's own.
    fn reaction_of(&self, choice: &Choice) -> Option<pb::Reaction> {
        if let Some(id) = choice.key.strip_prefix("c:") {
            let Some(Target::Channel { key, server, .. }) = self.target() else { return None };
            let emoji = self
                .core
                .shared
                .read(|s| s.instance(&key)?.emojis.get(&server)?.iter().find(|e| e.id == id).cloned())?;
            return Some(pb::Reaction {
                emoji_id: emoji.id,
                emoji_name: emoji.name,
                animated: emoji.animated,
                ..Default::default()
            });
        }
        crate::core::reactions::valid_emoji(&choice.insert)
            .then(|| pb::Reaction { emoji: choice.insert.clone(), ..Default::default() })
    }

    /// The emoji a message's menu offers to react with: the ones you used
    /// lately that work here, then the usual few.
    pub(crate) fn quick_reactions(&self, standard_only: bool) -> Vec<Choice> {
        let catalog = match self.target() {
            Some(Target::Channel { key, server, .. }) if !standard_only => self
                .core
                .shared
                .read(|s| {
                    let i = s.instance(&key)?;
                    Some(Catalog::own(i.server(&server)?, i.emojis.get(&server).map(Vec::as_slice).unwrap_or_default()))
                })
                .unwrap_or_default(),
            _ => Catalog::default(),
        };
        let prefs = self.core.prefs();
        let mut out: Vec<Choice> = Vec::new();
        let recent = prefs.recent_emoji.iter().filter_map(|k| Choice::recalled(k, &catalog, prefs.skin_tone));
        // The standard set writes some with a variation selector (👍️), as the web picks them.
        let usual = QUICK.iter().filter_map(|c| {
            Choice::recalled(&format!("u:{c}"), &catalog, prefs.skin_tone)
                .or_else(|| Choice::recalled(&format!("u:{c}\u{fe0f}"), &catalog, prefs.skin_tone))
        });
        for choice in recent.chain(usual) {
            if out.len() == QUICK.len() {
                break;
            }
            if !out.iter().any(|c| c.key == choice.key) {
                out.push(choice);
            }
        }
        out
    }

    /// Reacts to a message in the open place, or takes your reaction off.
    pub(crate) fn react_to(&mut self, msg: String, r: pb::Reaction, on: bool, cx: &mut Context<Self>) {
        let core = self.core.clone();
        match self.target() {
            Some(Target::Channel { key, server, channel }) => self.run(
                cx,
                async move { core.react(&key, &server, &channel, &msg, r, on).await },
                |this, result, cx| {
                    if let Err(err) = result {
                        let why = match err.code {
                            tonic::Code::ResourceExhausted => t("chattools.reactions.full"),
                            tonic::Code::FailedPrecondition => t("chattools.reactions.notHere"),
                            _ => err.message,
                        };
                        this.toast("circle-alert", t("chattools.reactions.failed"), why, None, None, cx);
                    }
                },
            ),
            Some(Target::Dm { key, conversation: room } | Target::Secure { key, channel: room, .. }) => {
                let Ok(seq) = msg.parse::<i64>() else { return };
                if r.emoji.is_empty() {
                    return;
                }
                self.run(cx, async move { core.react_dm(&key, &room, seq, &r.emoji, on).await }, |this, result, cx| {
                    if let Err(err) = result {
                        this.toast("circle-alert", t("chattools.reactions.failed"), err.0, None, None, cx);
                    }
                })
            }
            None => {}
        }
    }

    /// Reacts with an emoji from the menu's row (or takes it off, if it's already yours).
    pub(crate) fn react_with(&mut self, msg: String, choice: &Choice, cx: &mut Context<Self>) {
        let Some(mut r) = self.reaction_of(choice) else { return };
        // The same emoji with or without its variation selector (👍 and 👍️) joins the chip that's there.
        let there = self.message_reactions(&msg).and_then(|b| {
            b.chips.iter().find(|c| c.key.emoji_id == r.emoji_id && same_emoji(&c.key.emoji, &r.emoji)).cloned()
        });
        if let Some(c) = &there {
            r.emoji = c.key.emoji.clone();
        }
        let mine = there.is_some_and(|c| c.me);
        self.remember_emoji(&choice.key);
        self.react_to(msg, r, !mine, cx);
    }

    /// The reactions a message in the open list shows.
    fn message_reactions(&self, msg: &str) -> Option<Rc<ReactBits>> {
        self.rows.iter().chain(self.threads.rows.iter()).find_map(|row| match row {
            crate::ui::chat::Row::Msg(m) if m.id == msg => m.reactions.clone(),
            _ => None,
        })
    }

    fn remember_emoji(&self, key: &str) {
        let key = key.to_owned();
        self.core.set_prefs(|prefs| {
            prefs.recent_emoji.retain(|k| *k != key);
            prefs.recent_emoji.insert(0, key);
            prefs.recent_emoji.truncate(crate::core::config::MAX_RECENT_EMOJI);
        });
    }

    /// Opens the picker by `at` to react to a message.
    pub(crate) fn open_react_picker(
        &mut self,
        msg: String,
        standard_only: bool,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_context_menu(cx);
        self.open_emoji(window, cx);
        self.reacting = Some(ReactAt { msg, at, standard_only });
        cx.notify();
    }

    /// An emoji picked to react with: it goes, and the picker closes.
    pub(crate) fn react_with_picked(&mut self, choice: &Choice, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.reacting.clone() else { return };
        self.close_emoji(window, cx);
        if at.standard_only && choice.url.is_some() {
            return;
        }
        self.react_with(at.msg, choice, cx);
    }

    /// Takes every reaction off a message (Manage Messages).
    pub(crate) fn clear_all_reactions(&mut self, msg: String, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.clear_reactions(&key, &server, &channel, &msg, None).await },
            |this, result, cx| match result {
                Ok(()) => this.toast("check", t("chattools.reactions.cleared"), String::new(), None, None, cx),
                Err(err) => this.toast("circle-alert", t("chattools.reactions.failed"), err.message, None, None, cx),
            },
        );
    }

    /// Asks the instance who reacted with a chip's emoji, unless it's known at this count.
    pub(crate) fn load_reactors(&mut self, msg: String, chip: &Chip, cx: &mut Context<Self>) {
        let Some(Target::Channel { key, server, channel }) = self.target() else { return };
        let cache = reactors_key(&key, &msg, &chip.key);
        if self.reactors.get(&cache).is_some_and(|s| s.count == chip.count && !s.failed) {
            return;
        }
        let count = chip.count;
        self.reactors.insert(cache.clone(), SeenReactors { count, names: None, failed: false });
        let (core, emoji) = (self.core.clone(), chip.key.clone());
        let (k, sv) = (key.clone(), server.clone());
        self.run(
            cx,
            async move { core.list_reactors(&k, &sv, &channel, &msg, &emoji, "").await },
            move |this, result, cx| {
                let names = result.ok().map(|page| {
                    this.core
                        .shared
                        .read(|s| {
                            let i = s.instance(&key)?;
                            let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                            Some(
                                page.users
                                    .iter()
                                    .map(|u| {
                                        if u.id == me {
                                            t("chattools.reactions.you")
                                        } else {
                                            i.display_name(Some(&server), &u.id)
                                        }
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .unwrap_or_default()
                });
                let failed = names.is_none();
                this.reactors.insert(cache, SeenReactors { count, names, failed });
                cx.notify();
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(emoji: &str, count: u32) -> Chip {
        Chip {
            key: EmojiKey::standard(emoji),
            name: String::new(),
            animated: false,
            url: None,
            count,
            me: false,
            who: None,
        }
    }

    #[test]
    fn tooltips_name_a_few_then_count_the_rest() {
        let names: Vec<String> = ["Ann", "Bo", "Cy", "Di"].iter().map(|s| s.to_string()).collect();
        assert_eq!(people(&names[..2], 2), "Ann, Bo");
        assert_eq!(people(&names, 4), "Ann, Bo, Cy and 1 more");
        // The instance sent a page; the count says how many in all.
        assert_eq!(people(&names[..3], 10), "Ann, Bo, Cy and 7 more");
        let you = t("chattools.reactions.you");
        let text = tip_text(&chip("👍", 2), &["Ann".into(), you.clone()]);
        assert!(text.starts_with(&format!("{you}, Ann")), "{text}");
        assert!(text.contains("👍"));
    }

    #[test]
    fn variation_selectors_dont_split_an_emoji() {
        assert!(same_emoji("👍", "👍\u{fe0f}") && same_emoji("❤️", "❤") && !same_emoji("👍", "👎"));
    }

    #[test]
    fn rows_notice_what_came_and_went() {
        let mut seen = Seen::new(&[chip("👍", 1)]);
        seen.step(&[chip("👍", 2), chip("🎉", 1)], false);
        assert!(seen.born.contains_key("🎉") && !seen.born.contains_key("👍"));
        seen.step(&[chip("🎉", 1)], false);
        assert_eq!(seen.gone.len(), 1);
        assert_eq!(seen.gone[0].1.key.emoji, "👍");
        // With reduced motion nothing lingers.
        seen.step(&[], true);
        assert!(seen.gone.is_empty());
    }
}
