//! Boards (docs/boards.md), as the web app's `components/board/`: a board
//! channel opens as its columns side by side, scrolling sideways, each with
//! its cards scrolling up and down and "Add a card" at the bottom. A card
//! shows its labels, title, due date, comments, files and people; dragging
//! it (or its menu) moves it, within its column or to another. Opening a
//! card shows it beside the board: its column, people, labels, due date and
//! description, and the comments under it, which are its body's thread and
//! drawn as the thread panel draws replies. A filter bar narrows the board
//! to your cards, a label or some words, and "Archived cards" pages through
//! the ones taken off. What it holds is `core::board`.

use chrono::{Local, NaiveDate, TimeZone as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, DragMoveEvent, Entity, ExternalPaths, Focusable as _, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Rgba, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, rgb,
};

use crate::core::board::{self, Board, Due, Edit, Filter, Way};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::context_menu::{Built, Item, MenuOf, run};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner, mix, radius_xl};
use crate::ui::widgets::{avatar, header_button, icon, inner_scroll, pal, primary_button, soft_button};

/// A column's width, as the web's `w-72`.
const COLUMN: f32 = 288.0;
/// The card panel's width, as the thread panel's.
const PANEL: f32 = 440.0;

/// What the board on screen keeps between frames.
pub struct BoardUi {
    /// The board this is for ("key|channel"); another one starts over.
    pub here: Option<String>,
    /// The card open beside the board.
    pub open: Option<String>,
    /// The archived cards listed beside the board.
    pub archived: bool,
    pub filter: Filter,
    pub query: Entity<InputState>,
    /// The column a card is being added to, by its title.
    pub adding: Option<String>,
    pub new_title: Entity<InputState>,
    pub adding_busy: bool,
    /// The open card's title, being changed.
    pub renaming: bool,
    pub title: Entity<InputState>,
    /// The open card's description, being changed, and the files it keeps.
    pub describing: bool,
    pub body: Entity<TextareaState>,
    pub kept: Vec<pb::Attachment>,
    pub saving: bool,
    pub due: Entity<InputState>,
    /// "Delete this card?" under the open card's header.
    pub confirm_delete: bool,
    /// Where a card being dragged would land: (column, the card it goes above, "" for the bottom).
    pub drop: Option<(String, String)>,
}

impl BoardUi {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("chattools.board.filter")));
        let new_title = cx.new(|cx| InputState::new(window, cx).placeholder(t("chattools.board.newCardPlaceholder")));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(t("chattools.board.title")));
        let body = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(4, 14).placeholder(t("chattools.board.descriptionPlaceholder"))
        });
        let due = cx.new(|cx| InputState::new(window, cx).placeholder("yyyy-mm-dd"));
        let subs =
            vec![
                cx.subscribe_in(&query, window, |this: &mut FuwaApp, state, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        this.board.filter.query = state.read(cx).value().to_string();
                        cx.notify();
                    }
                }),
                cx.subscribe_in(&new_title, window, |this: &mut FuwaApp, state, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.add_card(window, cx),
                        InputEvent::Blur if state.read(cx).value().trim().is_empty() && !this.board.adding_busy => {
                            this.board.adding = None;
                            cx.notify();
                        }
                        _ => {}
                    }
                }),
                cx.subscribe_in(&title, window, |this: &mut FuwaApp, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.save_title(cx);
                    }
                }),
                cx.subscribe_in(&body, window, |this: &mut FuwaApp, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { secondary: true, .. } => this.save_description(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
                cx.subscribe_in(&due, window, |this: &mut FuwaApp, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_due(cx);
                    }
                }),
            ];
        (
            BoardUi {
                here: None,
                open: None,
                archived: false,
                filter: Filter::default(),
                query,
                adding: None,
                new_title,
                adding_busy: false,
                renaming: false,
                title,
                describing: false,
                body,
                kept: Vec::new(),
                saving: false,
                due,
                confirm_delete: false,
                drop: None,
            },
            subs,
        )
    }
}

/// A card being dragged, and the copy of it that follows the pointer.
#[derive(Clone)]
pub struct CardDrag {
    pub board: String,
    pub card: String,
    pub title: SharedString,
}

impl Render for CardDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        div()
            .w(px(COLUMN - 16.0))
            .p(px(10.0))
            .rounded(radius_xl())
            .bg(p.card)
            .border_1()
            .border_color(alpha(p.primary, 0.6))
            .rotate(gpui_kit::radians(2f32.to_radians()))
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, 0.3),
                offset: gpui_kit::point(px(0.0), px(12.0)),
                blur_radius: px(30.0),
                spread_radius: px(-10.0),
                inset: false,
            }])
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .text_color(p.foreground)
            .child(self.title.clone())
    }
}

/// What a board's menus are about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BoardMenu {
    /// A card's right-click menu.
    Card(String),
    /// The open card's column, people and labels.
    Column(String),
    People(String),
    Labels(String),
    /// The filter bar's label.
    FilterLabel,
}

/// `0xRRGGBB` as a colour; None for 0.
fn color_of(c: i32) -> Option<Rgba> {
    (c != 0).then(|| rgb(c as u32 & 0xff_ffff))
}

/// A label as a small chip in its colour.
fn label_chip(label: &pb::BoardLabel, p: &Palette) -> gpui_kit::Div {
    let color = color_of(label.color).unwrap_or(p.muted_foreground);
    div()
        .flex_none()
        .max_w_full()
        .px(px(8.0))
        .py(px(1.0))
        .rounded_full()
        .bg(alpha(color, 0.22))
        .text_color(mix(color, p.foreground, 0.25))
        .text_size(px(10.9))
        .font_weight(FontWeight::BOLD)
        .truncate()
        .child(label.name.clone())
}

/// "Oct 14", on this computer's clock.
fn short_day(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|at| at.format("%b %-d").to_string()).unwrap_or_default()
}

/// A day typed as yyyy-mm-dd, at noon there so it reads as the same day anywhere near.
fn day_at(day: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d").ok()?;
    Local.from_local_datetime(&date.and_hms_opt(12, 0, 0)?).earliest().map(|at| at.timestamp_millis())
}

fn day_of(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|at| at.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// A due date, red once it's past (unless the card is done).
fn due_chip(card: &pb::Card, column: Option<&pb::BoardColumn>, now: i64, p: &Palette) -> Option<gpui_kit::Div> {
    let state = board::due(card, column, now)?;
    let (bg, fg) = match state {
        Due::Late => (Some(alpha(p.destructive, 0.15)), Hsla::from(p.destructive)),
        Due::Soon => (Some(alpha(p.primary, 0.15)), Hsla::from(p.primary)),
        Due::Later => (None, Hsla::from(p.muted_foreground)),
    };
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .px(px(6.0))
            .py(px(1.0))
            .rounded_full()
            .when_some(bg, |el, bg| el.bg(bg))
            .text_color(fg)
            .when(state != Due::Later, |el| el.font_weight(FontWeight::BOLD))
            .child(icon("calendar").size(px(12.0)))
            .child(short_day(board::ms(card.due_at.as_ref()))),
    )
}

/// People on a card, overlapping.
fn faces(users: &[pb::User], max: usize, p: &Palette) -> gpui_kit::Div {
    let mut row = div().flex().flex_none();
    for (n, u) in users.iter().take(max).enumerate() {
        row = row.child(
            div().when(n > 0, |el| el.ml(px(-6.0))).rounded_full().border_2().border_color(p.card).child(avatar(
                Some(u),
                20.0,
                p,
            )),
        );
    }
    if users.len() > max {
        row = row.child(
            div()
                .ml(px(-6.0))
                .size(px(24.0))
                .rounded_full()
                .border_2()
                .border_color(p.card)
                .bg(p.muted)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.6))
                .font_weight(FontWeight::BOLD)
                .child(format!("+{}", users.len() - max)),
        );
    }
    row
}

/// What one frame of a board needs from the store.
struct Seen {
    board: Board,
    me: String,
    /// Move, edit and archive cards.
    manage: bool,
    /// Add cards (Manage Cards and Send Messages).
    add: bool,
    /// Delete anyone's card.
    delete_any: bool,
    attach: bool,
    /// Everyone the board names, by id.
    people: std::collections::HashMap<String, pb::User>,
}

impl FuwaApp {
    fn seen_board(&self, key: &str, server: &str, channel: &str) -> Option<Seen> {
        self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let board = i.boards.get(channel).cloned().unwrap_or_default();
            let access = i.access(server);
            let manage = access.has_in(channel, pb::Permission::ManageCards);
            let mut people = std::collections::HashMap::new();
            let ids = board
                .cards
                .iter()
                .chain(board.archived.iter().flat_map(|a| a.cards.iter()))
                .flat_map(|c| c.assignee_ids.iter());
            for id in ids {
                if let Some(u) = i.users.get(id) {
                    people.insert(id.clone(), u.clone());
                }
            }
            Some(Seen {
                me: i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default(),
                manage,
                add: manage && access.has_in(channel, pb::Permission::SendMessages) && !access.pending,
                delete_any: access.has_in(channel, pb::Permission::ManageMessages),
                attach: access.has_in(channel, pb::Permission::AttachFiles),
                board,
                people,
            })
        })
    }

    // ───────────────────────── Opening and moving ─────────────────────────

    /// Starts the board on screen over when it's another one, and reads it again.
    fn board_here(&mut self, key: &str, server: &str, channel: &str, window: &mut Window, cx: &mut Context<Self>) {
        let here = format!("{key}|{channel}");
        if self.board.here.as_deref() == Some(here.as_str()) {
            return;
        }
        self.board.here = Some(here);
        self.board.open = None;
        self.board.archived = false;
        self.board.filter = Filter::default();
        self.board.adding = None;
        self.board.renaming = false;
        self.board.describing = false;
        self.board.confirm_delete = false;
        self.board.drop = None;
        self.board.query.update(cx, |s, cx| s.set_value("", window, cx));
        let (core, key, server, channel) = (self.core.clone(), key.to_owned(), server.to_owned(), channel.to_owned());
        self.run(cx, async move { core.load_board(&key, &server, &channel).await }, |_, _, cx| cx.notify());
    }

    /// Leaving a board lets its screen go, so coming back reads it again.
    pub(crate) fn board_after_move(&mut self, cx: &mut Context<Self>) {
        let Some(here) = self.board.here.clone() else { return };
        let stays = matches!(self.target(), Some(crate::ui::app::Target::Channel { key, channel, .. })
            if here == format!("{key}|{channel}"));
        if !stays {
            self.board.here = None;
            if self.board.open.take().is_some() {
                self.close_thread(cx);
            }
        }
    }

    fn board_place(&self) -> Option<(String, String, String)> {
        let here = self.board.here.as_ref()?;
        match self.target()? {
            crate::ui::app::Target::Channel { key, server, channel } if *here == format!("{key}|{channel}") => {
                Some((key, server, channel))
            }
            _ => None,
        }
    }

    /// Opens a card beside the board, with its comments.
    pub(crate) fn open_card(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, _, channel)) = self.board_place() else { return };
        if self.board.open.as_deref() == Some(id.as_str()) {
            return;
        }
        // Its body is what its comments are under, read with the board.
        let body = self.core.shared.read(|s| s.instance(&key)?.boards.get(&channel)?.body(&id).cloned());
        let body = body.unwrap_or(pb::Message { id: id.clone(), channel_id: channel.clone(), ..Default::default() });
        self.core.shared.update(|s| {
            if let Some(i) = s.instances.get_mut(&key) {
                i.thread_parents.insert(id.clone(), body);
            }
        });
        self.board.open = Some(id.clone());
        self.board.archived = false;
        self.board.renaming = false;
        self.board.describing = false;
        self.board.confirm_delete = false;
        self.threads.reply.update(cx, |s, cx| s.set_placeholder(t("chattools.board.commentPlaceholder"), window, cx));
        self.open_thread(id, window, cx);
        cx.notify();
    }

    pub(crate) fn close_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.board.open = None;
        self.board.renaming = false;
        self.board.describing = false;
        self.board.confirm_delete = false;
        self.close_thread(cx);
        self.threads.reply.update(cx, |s, cx| s.set_placeholder("Reply in thread", window, cx));
        cx.notify();
    }

    fn toggle_archived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.board_place() else { return };
        let open = !self.board.archived || self.board.open.is_some();
        if self.board.open.is_some() {
            self.close_card(window, cx);
        }
        self.board.archived = open;
        if open {
            let core = self.core.clone();
            self.run(cx, async move { core.load_archived(&key, &server, &channel, false).await }, |_, _, cx| {
                cx.notify()
            });
        }
        cx.notify();
    }

    fn more_archived(&mut self, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.board_place() else { return };
        let core = self.core.clone();
        self.run(cx, async move { core.load_archived(&key, &server, &channel, true).await }, |_, _, cx| cx.notify());
    }

    fn board_failed(&mut self, what: Result<(), crate::core::api::Problem>, cx: &mut Context<Self>) {
        if let Err(err) = what {
            self.toast("circle-alert", "Couldn't change that".into(), err.message, None, None, cx);
        }
        cx.notify();
    }

    pub(crate) fn move_card_to(&mut self, card: &str, column: &str, before: &str, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.board_place() else { return };
        let (card, column, before) = (card.to_owned(), column.to_owned(), before.to_owned());
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.move_card(&key, &server, &channel, &card, &column, &before).await },
            |this, result, cx| this.board_failed(result, cx),
        );
        cx.notify();
    }

    fn step_card(&mut self, card: &str, way: Way, cx: &mut Context<Self>) {
        let Some((key, _, channel)) = self.board_place() else { return };
        let to = self.core.shared.read(|s| board::step(s.instance(&key)?.boards.get(&channel)?, card, way));
        if let Some((column, before)) = to {
            self.move_card_to(card, &column, &before, cx);
        }
    }

    pub(crate) fn change_card(&mut self, card: &str, edit: Edit, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.board_place() else { return };
        let card = card.to_owned();
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.update_card(&key, &server, &channel, &card, edit).await },
            |this, result, cx| this.board_failed(result, cx),
        );
        cx.notify();
    }

    fn delete_card(&mut self, card: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.board_place() else { return };
        if self.board.open.as_deref() == Some(card) {
            self.close_card(window, cx);
        }
        let card = card.to_owned();
        let core = self.core.clone();
        self.run(cx, async move { core.delete_message(&key, &server, &channel, &card).await }, |this, result, cx| {
            this.board_failed(result, cx)
        });
    }

    fn start_adding(&mut self, column: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.board.adding = Some(column.to_owned());
        self.board.new_title.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn add_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.board_place() else { return };
        let Some(column) = self.board.adding.clone() else { return };
        let title: String =
            self.board.new_title.read(cx).value().trim().chars().take(board::TITLE_MAX).collect::<String>();
        if title.is_empty() || self.board.adding_busy {
            return;
        }
        self.board.adding_busy = true;
        self.board.new_title.update(cx, |s, cx| s.set_value("", window, cx));
        let core = self.core.clone();
        self.run(
            cx,
            async move { core.create_card(&key, &server, &channel, &column, &title).await },
            |this, result, cx| {
                this.board.adding_busy = false;
                this.board_failed(result, cx);
            },
        );
        cx.notify();
    }

    fn start_renaming(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.board.renaming = true;
        self.board.title.update(cx, |s, cx| {
            s.set_value(title.to_owned(), window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn save_title(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.board.renaming) {
            return;
        }
        let Some(card) = self.board.open.clone() else { return };
        let title: String = self.board.title.read(cx).value().trim().chars().take(board::TITLE_MAX).collect();
        let had = self.open_card_now().map(|c| c.title).unwrap_or_default();
        if !title.is_empty() && title != had {
            self.change_card(&card, Edit { title: Some(title), ..Default::default() }, cx);
        }
        cx.notify();
    }

    fn save_due(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.board.open.clone() else { return };
        let typed = self.board.due.read(cx).value().to_string();
        match day_at(&typed) {
            Some(at) => self.change_card(&card, Edit { due: Some(at), ..Default::default() }, cx),
            None if typed.trim().is_empty() => self.change_card(&card, Edit { due: Some(0), ..Default::default() }, cx),
            None => self.toast("calendar", "That isn't a day".into(), "Write it as yyyy-mm-dd.".into(), None, None, cx),
        }
    }

    fn start_describing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, _, channel)) = self.board_place() else { return };
        let Some(card) = self.board.open.clone() else { return };
        let body = self.core.shared.read(|s| s.instance(&key)?.boards.get(&channel)?.body(&card).cloned());
        let body = body.unwrap_or_default();
        self.board.describing = true;
        self.board.kept = body.attachments.clone();
        self.board.body.update(cx, |s, cx| {
            s.set_value(body.content.clone(), window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn save_description(&mut self, cx: &mut Context<Self>) {
        if !self.board.describing || self.board.saving {
            return;
        }
        let Some(card) = self.board.open.clone() else { return };
        let added = match self.take_files() {
            Ok(files) => files,
            Err(why) => {
                self.toast("paperclip", why.into(), String::new(), None, None, cx);
                return;
            }
        };
        let text: String = self.board.body.read(cx).value().chars().take(board::BODY_MAX).collect();
        let mut files = std::mem::take(&mut self.board.kept);
        files.extend(added);
        self.board.describing = false;
        self.change_card(&card, Edit { body: Some((text, files)), ..Default::default() }, cx);
    }

    /// The open card, on the board or among the archived.
    fn open_card_now(&self) -> Option<pb::Card> {
        let (key, _, channel) = self.board_place()?;
        let card = self.board.open.as_ref()?;
        self.core.shared.read(|s| s.instance(&key)?.boards.get(&channel)?.card(card).cloned())
    }

    /// Escape leaves what's being typed on the board; in the comment box it closes the card.
    pub(crate) fn board_keys(
        &mut self,
        key: &gpui_kit::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.key != "escape" || self.board.here.is_none() {
            return false;
        }
        let focused =
            |state: &Entity<InputState>, cx: &Context<Self>| state.read(cx).focus_handle(cx).is_focused(window);
        if self.board.adding.is_some() && focused(&self.board.new_title, cx) {
            self.board.adding = None;
        } else if self.board.renaming && focused(&self.board.title, cx) {
            self.board.renaming = false;
        } else if self.board.describing && self.board.body.read(cx).focus_handle(cx).is_focused(window) {
            self.board.describing = false;
        } else if self.board.open.is_some() && self.threads.reply.read(cx).focus_handle(cx).is_focused(window) {
            self.close_card(window, cx);
        } else {
            return false;
        }
        cx.notify();
        true
    }

    // ───────────────────────── Menus ─────────────────────────

    pub(crate) fn board_menu_items(&self, menu: &BoardMenu) -> Built {
        let Some((key, server, channel)) = self.board_place() else { return Built::of(Vec::new()) };
        let Some(seen) = self.seen_board(&key, &server, &channel) else { return Built::of(Vec::new()) };
        let b = &seen.board;
        match menu {
            BoardMenu::Card(id) => {
                let Some(card) = b.card(id).cloned() else { return Built::of(Vec::new()) };
                let open_id = id.clone();
                let open = vec![Item::act(
                    t("chat.threads.open"),
                    "panel-right-open",
                    run(move |this, window, cx| this.open_card(open_id.clone(), window, cx)),
                )];
                let mut moves = Vec::new();
                if seen.manage && card.archived_at.is_none() {
                    for (way, label, glyph) in [
                        (Way::Up, t("chattools.board.moveUp"), "arrow-up"),
                        (Way::Down, t("chattools.board.moveDown"), "arrow-down"),
                    ] {
                        let id = id.clone();
                        moves.push(
                            Item::act(label, glyph, run(move |this, _, cx| this.step_card(&id, way, cx)))
                                .disabled(board::step(b, &card.id, way).is_none()),
                        );
                    }
                    let columns = b
                        .columns
                        .iter()
                        .map(|c| {
                            let (id, column) = (id.clone(), c.id.clone());
                            Item::check(
                                c.name.clone(),
                                c.id == card.column_id,
                                true,
                                run(move |this, _, cx| this.move_card_to(&id, &column, "", cx)),
                            )
                            .colored(color_of(c.color).map(Into::into))
                        })
                        .collect();
                    moves.push(Item::sub(t("chattools.board.column"), "columns-3", columns));
                }
                let mut end = Vec::new();
                if seen.manage {
                    let (id, archived) = (id.clone(), card.archived_at.is_some());
                    end.push(Item::act(
                        if archived { t("chattools.board.restore") } else { t("chattools.board.archive") },
                        if archived { "archive-restore" } else { "archive" },
                        run(move |this, _, cx| {
                            this.change_card(&id, Edit { archived: Some(!archived), ..Default::default() }, cx)
                        }),
                    ));
                }
                if seen.delete_any || card.author_id == seen.me {
                    let id = id.clone();
                    end.push(
                        Item::act(
                            t("chattools.board.delete"),
                            "trash-2",
                            run(move |this, window, cx| this.delete_card(&id, window, cx)),
                        )
                        .danger()
                        .confirm(
                            t("chattools.board.delete"),
                            &t("chattools.board.deleteConfirm"),
                            &t("chattools.board.delete"),
                        ),
                    );
                }
                Built::of(vec![open, moves, end])
            }
            BoardMenu::Column(id) => {
                let Some(card) = b.card(id) else { return Built::of(Vec::new()) };
                let items = b
                    .columns
                    .iter()
                    .map(|c| {
                        let (id, column, here) = (id.clone(), c.id.clone(), c.id == card.column_id);
                        Item::check(
                            c.name.clone(),
                            here,
                            true,
                            run(move |this, _, cx| {
                                if !here {
                                    this.move_card_to(&id, &column, "", cx);
                                }
                            }),
                        )
                    })
                    .collect();
                Built::of(vec![items])
            }
            BoardMenu::People(id) => {
                let Some(card) = b.card(id) else { return Built::of(Vec::new()) };
                let mut members: Vec<(String, String)> = self.core.shared.read(|s| {
                    let Some(i) = s.instance(&key) else { return Vec::new() };
                    i.members
                        .get(&server)
                        .into_iter()
                        .flatten()
                        .filter_map(|m| m.user.as_ref())
                        .map(|u| (u.id.clone(), i.display_name(Some(&server), &u.id)))
                        .collect()
                });
                members.sort_by_key(|(_, name)| name.to_lowercase());
                let value = card.assignee_ids.clone();
                let items = members
                    .into_iter()
                    .map(|(user, name)| {
                        let on = value.contains(&user);
                        let full = !on && value.len() >= board::ASSIGNEES_MAX;
                        let (id, value) = (id.clone(), value.clone());
                        Item::check(
                            name,
                            on,
                            false,
                            run(move |this, _, cx| {
                                let mut next = value.clone();
                                if on {
                                    next.retain(|u| *u != user);
                                } else {
                                    next.push(user.clone());
                                }
                                this.change_card(&id, Edit { assignees: Some(next), ..Default::default() }, cx)
                            }),
                        )
                        .disabled(full)
                    })
                    .collect();
                Built::of(vec![items])
            }
            BoardMenu::Labels(id) => {
                let Some(card) = b.card(id) else { return Built::of(Vec::new()) };
                let value = card.label_ids.clone();
                let items = b
                    .labels
                    .iter()
                    .map(|l| {
                        let on = value.contains(&l.id);
                        let (id, label, value) = (id.clone(), l.id.clone(), value.clone());
                        Item::check(
                            l.name.clone(),
                            on,
                            false,
                            run(move |this, _, cx| {
                                let mut next = value.clone();
                                if on {
                                    next.retain(|x| *x != label);
                                } else {
                                    next.push(label.clone());
                                }
                                this.change_card(&id, Edit { labels: Some(next), ..Default::default() }, cx)
                            }),
                        )
                        .colored(color_of(l.color).map(Into::into))
                    })
                    .collect();
                Built::of(vec![items])
            }
            BoardMenu::FilterLabel => {
                let picked = self.board.filter.label.clone();
                let any = Item::check(
                    t("chattools.board.anyLabel"),
                    picked.is_none(),
                    true,
                    run(|this, _, cx| {
                        this.board.filter.label = None;
                        cx.notify();
                    }),
                );
                let labels = b
                    .labels
                    .iter()
                    .map(|l| {
                        let id = l.id.clone();
                        Item::check(
                            l.name.clone(),
                            picked.as_ref() == Some(&l.id),
                            true,
                            run(move |this, _, cx| {
                                this.board.filter.label = Some(id.clone());
                                cx.notify();
                            }),
                        )
                        .colored(color_of(l.color).map(Into::into))
                    })
                    .collect();
                Built::of(vec![vec![any], labels])
            }
        }
    }

    fn board_menu_at(
        &mut self,
        menu: BoardMenu,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, _, channel)) = self.board_place() else { return };
        self.open_context_menu(MenuOf::Board { channel, menu }, at, window, cx);
    }

    // ───────────────────────── Drawing ─────────────────────────

    /// A board channel, where the message list would be.
    pub(crate) fn board_view(
        &mut self,
        key: &str,
        server: &str,
        channel: &pb::Channel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        self.board_here(key, server, &channel.id, window, cx);
        let Some(seen) = self.seen_board(key, server, &channel.id) else { return div().into_any_element() };
        // The open card was deleted, or archived away from the list it was opened from.
        if let Some(open) = self.board.open.clone()
            && seen.board.loaded
            && seen.board.card(&open).is_none()
        {
            self.close_card(window, cx);
        }
        let now = crate::core::dms::now_ms();

        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("square-kanban").size(px(20.0)).text_color(p.muted_foreground))
            .child(
                div().flex_none().font_weight(FontWeight::EXTRA_BOLD).text_size(px(16.0)).line_height(px(24.0)).child(
                    motion::swap_text(
                        SharedString::from(format!("channel-name|{}", channel.id)),
                        channel.name.clone(),
                        16.0,
                        window,
                        cx,
                    ),
                ),
            )
            .when(!channel.topic.is_empty(), |el| {
                el.child(div().flex_none().w(px(1.0)).h(px(20.0)).bg(p.border)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(channel.topic.clone()),
                )
            })
            .when(channel.topic.is_empty(), |el| el.child(div().flex_1()))
            .child(self.bell_button(key, server, &channel.id, cx))
            .child({
                let on = self.board.archived && self.board.open.is_none();
                header_button("board-archived", "archive", on, -8.0, &p)
                    .tooltip(|window, cx| crate::ui::overlay::Tip::new(t("chattools.board.archived")).build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_archived(window, cx)))
            });

        let body = if let (Some(error), false) = (&seen.board.error, seen.board.loaded) {
            div()
                .m(px(16.0))
                .p(px(12.0))
                .rounded(radius_xl())
                .bg(alpha(p.destructive, 0.1))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(p.destructive)
                .child(error.clone())
                .into_any_element()
        } else {
            let mut row = div()
                .id(SharedString::from(format!("board|{}", channel.id)))
                .flex_1()
                .min_h_0()
                .overflow_x_scroll()
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(16.0));
            if !seen.board.loaded {
                for n in 0..3 {
                    row = row.child(
                        div()
                            .flex_none()
                            .w(px(COLUMN))
                            .h(px(256.0))
                            .rounded(corner(16.0))
                            .bg(alpha(p.muted, 0.5 - 0.1 * n as f32)),
                    );
                }
            } else {
                for column in &seen.board.columns {
                    row = row.child(self.board_column(column, &seen, now, &p, window, cx));
                }
            }
            row.into_any_element()
        };

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(self.board_filters(&seen, &p, window, cx))
            .child(body);
        let side = if self.board.open.is_some() {
            self.card_panel(&seen, now, &p, window, cx)
        } else if self.board.archived {
            Some(self.archived_panel(&seen, &p, window, cx))
        } else {
            None
        };
        // No member list beside a board: its people aren't on screen.
        self.core.set_in_view(key, server, &[]);
        div().size_full().relative().flex().child(main).children(side).into_any_element()
    }

    /// Narrowing the board: words, your cards, a label.
    fn board_filters(&mut self, seen: &Seen, p: &Palette, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        let filter = self.board.filter.clone();
        let chip = |id: &'static str, on: bool| {
            let hover = p.foreground;
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.0))
                .h(px(32.0))
                .px(px(12.0))
                .rounded_full()
                .border_1()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .active(|s| s.scale(0.95))
                .map(|el| {
                    if on {
                        el.border_color(alpha(p.primary, 0.6)).bg(alpha(p.primary, 0.15)).text_color(p.primary)
                    } else {
                        el.border_color(p.border)
                            .bg(p.card)
                            .text_color(p.muted_foreground)
                            .hover(move |s| s.text_color(hover))
                    }
                })
        };
        let me = seen.me.clone();
        let mine = filter.assignee.as_deref() == Some(me.as_str()) && !me.is_empty();
        let label = filter.label.as_ref().and_then(|id| seen.board.labels.iter().find(|l| &l.id == id));
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(32.0))
                    .w(px(256.0))
                    .px(px(12.0))
                    .rounded_full()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .text_sm()
                    .child(icon("search").size(px(14.0)).text_color(p.muted_foreground))
                    .child(
                        div().flex_1().min_w_0().ml(px(-10.0)).child(Input::new(&self.board.query).appearance(false)),
                    ),
            )
            .child(
                chip("board-mine", mine)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.board.filter.assignee = if mine { None } else { Some(me.clone()) };
                        cx.notify();
                    }))
                    .child(icon("user").size(px(14.0)))
                    .child(t("chattools.board.mine")),
            )
            .when(!seen.board.labels.is_empty(), |el| {
                el.child(
                    chip("board-label", label.is_some())
                        .on_click(cx.listener(|this, ev: &gpui_kit::ClickEvent, window, cx| {
                            this.board_menu_at(BoardMenu::FilterLabel, ev.position(), window, cx)
                        }))
                        .child(icon("tag").size(px(14.0)))
                        .child(match label {
                            Some(l) => label_chip(l, &p).into_any_element(),
                            None => t("chattools.board.anyLabel").into_any_element(),
                        }),
                )
            })
            .when(filter.any(), |el| {
                el.child(
                    div()
                        .id("board-clear")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(32.0))
                        .px(px(12.0))
                        .rounded_full()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(p.muted).text_color(p.foreground))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.board.filter = Filter::default();
                            this.board.query.update(cx, |s, cx| s.set_value("", window, cx));
                            cx.notify();
                        }))
                        .child(icon("filter-x").size(px(14.0)))
                        .child(t("chattools.board.clearFilters")),
                )
            })
            .into_any_element()
    }

    fn board_column(
        &mut self,
        column: &pb::BoardColumn,
        seen: &Seen,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let b = &seen.board;
        let filtering = self.board.filter.any();
        let total = b.cards_in(&column.id).count();
        let over = b.over_limit(column);
        let cards: Vec<&pb::Card> = b
            .cards_in(&column.id)
            .filter(|c| !filtering || board::shows(c, b.bodies.get(&c.id), &self.board.filter))
            .collect();
        let add = seen.add && !filtering;
        let adding = add && self.board.adding.as_deref() == Some(column.id.as_str());
        let dragging = cx.has_active_drag();
        let drop = self.board.drop.clone().filter(|(c, _)| dragging && *c == column.id);
        let line = || div().h(px(2.0)).mx(px(4.0)).rounded_full().bg(p.primary);

        let dot = color_of(column.color).unwrap_or(p.muted_foreground);
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .pt(px(12.0))
            .pb(px(8.0))
            .child(div().size(px(10.0)).flex_none().rounded_full().bg(dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(column.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(8.0))
                    .py(px(1.0))
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if over { p.destructive } else { p.muted_foreground })
                    .when(over, |el| el.bg(alpha(p.destructive, 0.15)))
                    .child(if column.card_limit > 0 {
                        format!("{total} / {}", column.card_limit)
                    } else {
                        total.to_string()
                    }),
            )
            .when(add, |el| {
                let id = column.id.clone();
                el.child(
                    div()
                        .id(SharedString::from(format!("col-add|{}", column.id)))
                        .size(px(28.0))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(p.muted).text_color(p.foreground))
                        .tooltip({
                            let tip = t_with("chattools.board.addCardTo", &[("column", Arg::Str(&column.name))]);
                            move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| this.start_adding(&id, window, cx)))
                        .child(icon("plus").size(px(16.0))),
                )
            });

        let mut list = inner_scroll(format!("col-cards|{}", column.id), window, cx)
            .flex_shrink(1.0)
            .min_h(px(48.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(8.0))
            .pb(px(8.0));
        for (n, card) in cards.iter().enumerate() {
            if drop.as_ref().is_some_and(|(_, before)| *before == card.id) {
                list = list.child(line());
            }
            let next = cards.get(n + 1).map(|c| c.id.clone()).unwrap_or_default();
            list = list.child(self.board_card(card, column, seen, &next, now, &p, window, cx));
        }
        if drop.as_ref().is_some_and(|(_, before)| before.is_empty()) {
            list = list.child(line());
        }
        if cards.is_empty() && !adding {
            list = list.child(
                div()
                    .px(px(8.0))
                    .py(px(12.0))
                    .text_center()
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(t("chattools.board.emptyColumn")),
            );
        }

        let foot = add.then(|| {
            if adding {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .pb(px(8.0))
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(radius_xl())
                            .border_1()
                            .border_color(alpha(p.primary, 0.6))
                            .bg(p.card)
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(div().ml(px(-10.0)).child(Input::new(&self.board.new_title).appearance(false))),
                    )
                    .child(
                        div()
                            .px(px(4.0))
                            .text_size(px(10.9))
                            .text_color(p.muted_foreground)
                            .child(t("chattools.board.newCardHint")),
                    )
                    .into_any_element()
            } else {
                let id = column.id.clone();
                div()
                    .px(px(8.0))
                    .pb(px(8.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("col-add-foot|{}", column.id)))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(radius_xl())
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .hover(move |s| s.bg(p.muted).text_color(p.foreground))
                            .on_click(cx.listener(move |this, _, window, cx| this.start_adding(&id, window, cx)))
                            .child(icon("plus").size(px(16.0)))
                            .child(t("chattools.board.addCard")),
                    )
                    .into_any_element()
            }
        });

        let column_id = column.id.clone();
        let over_column = column.id.clone();
        let board_id = self.board.here.clone().unwrap_or_default();
        div()
            .id(SharedString::from(format!("col|{}", column.id)))
            .flex_none()
            .w(px(COLUMN))
            .max_h_full()
            .flex()
            .flex_col()
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .when(seen.manage, |el| {
                let board_id = board_id.clone();
                // Over the column but past its cards: the bottom (a card's own listener says otherwise).
                el.on_drag_move::<CardDrag>(cx.listener(move |this, ev: &DragMoveEvent<CardDrag>, _, cx| {
                    if ev.drag(cx).board == board_id && ev.bounds.contains(&ev.event.position) {
                        let next = Some((over_column.clone(), String::new()));
                        if this.board.drop != next {
                            this.board.drop = next;
                            cx.notify();
                        }
                    }
                }))
                .on_drop::<CardDrag>(cx.listener(move |this, drag: &CardDrag, _, cx| {
                    let (column, before) = this.board.drop.take().unwrap_or_else(|| (column_id.clone(), String::new()));
                    if before != drag.card {
                        this.move_card_to(&drag.card, &column, &before, cx);
                    }
                }))
            })
            .child(head)
            .child(list)
            .children(foot)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn board_card(
        &mut self,
        card: &pb::Card,
        column: &pb::BoardColumn,
        seen: &Seen,
        next: &str,
        now: i64,
        p: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let b = &seen.board;
        let body = b.bodies.get(&card.id);
        let comments = body.and_then(|m| m.thread.as_ref()).map_or(0, |t| t.reply_count);
        let files = body.map_or(0, |m| m.attachments.len());
        let labels: Vec<&pb::BoardLabel> = b.labels.iter().filter(|l| card.label_ids.contains(&l.id)).collect();
        let people: Vec<pb::User> = card.assignee_ids.iter().filter_map(|id| seen.people.get(id).cloned()).collect();
        let selected = self.board.open.as_deref() == Some(card.id.as_str());
        let due = due_chip(card, Some(column), now, &p);
        let meta = due.is_some() || comments > 0 || files > 0 || !people.is_empty();
        let hover_border = alpha(p.primary, 0.5);
        let id = card.id.clone();
        let menu_id = card.id.clone();
        let board_id = self.board.here.clone().unwrap_or_default();
        let (column_id, above, below) = (column.id.clone(), card.id.clone(), next.to_owned());
        let drag = CardDrag { board: board_id.clone(), card: card.id.clone(), title: card.title.clone().into() };
        div()
            .id(SharedString::from(format!("card|{}", card.id)))
            .flex_none()
            .p(px(10.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(if selected { alpha(p.primary, 0.7) } else { p.border.into() })
            .bg(p.card)
            .when(selected, |el| {
                el.shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.3),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(2.0),
                    inset: false,
                }])
            })
            .cursor_pointer()
            .hover(move |s| s.border_color(hover_border).translate_y(px(-2.0)))
            .on_click(cx.listener(move |this, _, window, cx| this.open_card(id.clone(), window, cx)))
            .on_mouse_down(
                gpui_kit::MouseButton::Right,
                cx.listener(move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.board_menu_at(BoardMenu::Card(menu_id.clone()), ev.position, window, cx);
                }),
            )
            .when(seen.manage && card.archived_at.is_none(), |el| {
                el.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())).on_drag_move::<CardDrag>(cx.listener(
                    move |this, ev: &DragMoveEvent<CardDrag>, _, cx| {
                        if ev.drag(cx).board != board_id || !ev.bounds.contains(&ev.event.position) {
                            return;
                        }
                        // The upper half goes above this card, the lower half below it.
                        let upper = ev.event.position.y < ev.bounds.center().y;
                        let next = Some((column_id.clone(), if upper { above.clone() } else { below.clone() }));
                        if this.board.drop != next {
                            this.board.drop = next;
                            cx.notify();
                        }
                    },
                ))
            })
            .when(!labels.is_empty(), |el| {
                el.child(
                    div()
                        .mb(px(6.0))
                        .flex()
                        .flex_wrap()
                        .gap(px(4.0))
                        .children(labels.iter().map(|l| label_chip(l, &p))),
                )
            })
            .child(div().text_sm().line_height(px(19.0)).font_weight(FontWeight::BOLD).child(card.title.clone()))
            .when(meta, |el| {
                el.child(
                    div()
                        .mt(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(11.2))
                        .text_color(p.muted_foreground)
                        .children(due)
                        .when(comments > 0, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(icon("message-square").size(px(12.0)))
                                    .child(comments.to_string()),
                            )
                        })
                        .when(files > 0, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(icon("paperclip").size(px(12.0)))
                                    .child(files.to_string()),
                            )
                        })
                        .child(div().flex_1())
                        .child(faces(&people, 3, &p)),
                )
            })
            .into_any_element()
    }

    /// The open card beside the board: its fields, description and comments.
    fn card_panel(
        &mut self,
        seen: &Seen,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = *p;
        let id = self.board.open.clone()?;
        let b = &seen.board;
        let card = b.card(&id)?.clone();
        let body = b.body(&id).cloned().unwrap_or_default();
        let column = b.columns.iter().find(|c| c.id == card.column_id);
        let archived = card.archived_at.is_some();
        let manage = seen.manage;
        let can_delete = seen.delete_any || card.author_id == seen.me;
        let key = self.board.here.clone().and_then(|h| h.split('|').next().map(str::to_owned)).unwrap_or_default();

        let column_name = column.map(|c| c.name.clone()).unwrap_or_default();
        let where_text = if archived {
            t_with("chattools.board.archivedIn", &[("column", Arg::Str(&column_name))])
        } else {
            column_name.clone()
        };
        let dot = column.and_then(|c| color_of(c.color)).unwrap_or(p.muted_foreground);
        let panel_button = |id: &'static str, glyph: &'static str, label: String, danger: bool| {
            let hover_fg = if danger { p.destructive } else { p.foreground };
            let hover_bg = p.muted;
            div()
                .id(id)
                .size(px(36.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(p.muted_foreground)
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .active(|s| s.scale(0.85))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
                .child(icon(glyph).size(px(18.0)))
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .h(px(56.0))
            .border_b_1()
            .border_color(p.border)
            .child(div().size(px(10.0)).flex_none().rounded_full().bg(dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(where_text),
            )
            .when(manage, |el| {
                let (id, label) = (
                    card.id.clone(),
                    if archived { t("chattools.board.restore") } else { t("chattools.board.archive") },
                );
                el.child(
                    panel_button("card-archive", if archived { "archive-restore" } else { "archive" }, label, false)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_card(&id, Edit { archived: Some(!archived), ..Default::default() }, cx)
                        })),
                )
            })
            .when(can_delete, |el| {
                el.child(panel_button("card-delete", "trash-2", t("chattools.board.delete"), true).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.board.confirm_delete = true;
                        cx.notify();
                    }),
                ))
            })
            .child(
                panel_button("card-close", "x", t("chattools.board.close"), false)
                    .on_click(cx.listener(|this, _, window, cx| this.close_card(window, cx))),
            );

        let confirm = self.board.confirm_delete.then(|| {
            let id = card.id.clone();
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(8.0))
                .border_b_1()
                .border_color(p.border)
                .bg(alpha(p.destructive, 0.1))
                .text_sm()
                .child(div().flex_1().font_weight(FontWeight::BOLD).child(t("chattools.board.deleteConfirm")))
                .child(soft_button("card-delete-no", t("chattools.board.cancel"), &p).h(px(32.0)).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.board.confirm_delete = false;
                        cx.notify();
                    }),
                ))
                .child(
                    div()
                        .id("card-delete-yes")
                        .h(px(32.0))
                        .px(px(12.0))
                        .rounded(corner(10.0))
                        .flex()
                        .items_center()
                        .bg(p.destructive)
                        .text_color(gpui_kit::white())
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| this.delete_card(&id, window, cx)))
                        .child(t("chattools.board.delete")),
                )
        });

        // The title, edited in place.
        let title = if self.board.renaming {
            div()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(alpha(p.primary, 0.6))
                .bg(p.card)
                .text_size(px(18.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(div().ml(px(-10.0)).child(Input::new(&self.board.title).appearance(false)))
                .into_any_element()
        } else {
            let title = card.title.clone();
            div()
                .id("card-title")
                .group("card-title")
                .flex()
                .items_start()
                .gap(px(8.0))
                .text_size(px(18.0))
                .line_height(px(24.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(div().flex_1().min_w_0().child(card.title.clone()))
                .when(manage, |el| {
                    el.child(
                        div()
                            .id("card-rename")
                            .mt(px(4.0))
                            .p(px(4.0))
                            .rounded(corner(6.0))
                            .text_color(p.muted_foreground)
                            .opacity(0.0)
                            .group_hover("card-title", |s| s.opacity(1.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(p.muted))
                            .tooltip(|window, cx| {
                                crate::ui::overlay::Tip::new(t("chattools.board.rename")).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| this.start_renaming(&title, window, cx)))
                            .child(icon("pencil").size(px(14.0))),
                    )
                })
                .into_any_element()
        };

        let field = |glyph: &'static str, label: String, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .min_h(px(28.0))
                .child(
                    div()
                        .w(px(84.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(icon(glyph).size(px(14.0)))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let plus = |id: &'static str, label: String| {
            let hover = p.primary;
            div()
                .id(id)
                .size(px(24.0))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted_foreground)
                .cursor_pointer()
                .hover(move |s| s.border_color(hover).text_color(hover))
                .tooltip(move |window, cx| crate::ui::overlay::Tip::new(label.clone()).build(window, cx))
                .child(icon("plus").size(px(14.0)))
        };

        let column_value = {
            let chip = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child(div().size(px(8.0)).rounded_full().bg(dot))
                .child(column_name.clone());
            if manage && !archived {
                let id = card.id.clone();
                div()
                    .id("card-column")
                    .px(px(8.0))
                    .py(px(4.0))
                    .ml(px(-8.0))
                    .rounded(corner(8.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.muted))
                    .on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                        this.board_menu_at(BoardMenu::Column(id.clone()), ev.position(), window, cx)
                    }))
                    .child(chip)
                    .into_any_element()
            } else {
                chip.into_any_element()
            }
        };

        let people: Vec<pb::User> = card.assignee_ids.iter().filter_map(|id| seen.people.get(id).cloned()).collect();
        let people_value = {
            let mut row = div().flex().flex_wrap().items_center().gap(px(6.0));
            for u in &people {
                let name = self
                    .core
                    .shared
                    .read(|s| s.instance(&key).map(|i| i.display_name(None, &u.id)))
                    .unwrap_or_else(|| u.username.clone());
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .pl(px(2.0))
                        .pr(px(8.0))
                        .py(px(2.0))
                        .rounded_full()
                        .bg(p.muted)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .child(avatar(Some(u), 20.0, &p))
                        .child(name),
                );
            }
            if people.is_empty() {
                row = row.child(div().text_sm().text_color(p.muted_foreground).child(t("chattools.board.nobody")));
            }
            if manage {
                let id = card.id.clone();
                row = row.child(plus("card-people", t("chattools.board.addPeople")).on_click(cx.listener(
                    move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                        this.board_menu_at(BoardMenu::People(id.clone()), ev.position(), window, cx)
                    },
                )));
            }
            row.into_any_element()
        };

        let labels_value = {
            let shown: Vec<&pb::BoardLabel> = b.labels.iter().filter(|l| card.label_ids.contains(&l.id)).collect();
            let mut row = div().flex().flex_wrap().items_center().gap(px(4.0));
            for l in &shown {
                row = row.child(label_chip(l, &p));
            }
            if shown.is_empty() {
                let none = if b.labels.is_empty() {
                    t("chattools.board.noBoardLabels")
                } else {
                    t("chattools.board.noLabels")
                };
                row = row.child(div().text_sm().text_color(p.muted_foreground).child(none));
            }
            if manage && !b.labels.is_empty() {
                let id = card.id.clone();
                row = row.child(plus("card-labels", t("chattools.board.addLabels")).on_click(cx.listener(
                    move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                        this.board_menu_at(BoardMenu::Labels(id.clone()), ev.position(), window, cx)
                    },
                )));
            }
            row.into_any_element()
        };

        let due_value = if manage {
            let typed = self.board.due.read(cx).value().to_string();
            let saved = card.due_at.as_ref().map(|t| day_of(board::ms(Some(t)))).unwrap_or_default();
            // The field follows the card until someone types in it.
            if !self.board.due.read(cx).focus_handle(cx).is_focused(window) && typed != saved {
                self.board.due.update(cx, |s, cx| s.set_value(saved.clone(), window, cx));
            }
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(132.0))
                        .px(px(8.0))
                        .rounded(corner(8.0))
                        .border_1()
                        .border_color(p.border)
                        .bg(p.card)
                        .text_sm()
                        .child(div().ml(px(-10.0)).child(Input::new(&self.board.due).appearance(false))),
                )
                .children(due_chip(&card, column, now, &p).filter(|_| card.due_at.is_some()).map(|c| c.text_xs()))
                .when(card.due_at.is_some(), |el| {
                    let id = card.id.clone();
                    let hover = p.destructive;
                    el.child(
                        div()
                            .id("card-due-clear")
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .hover(move |s| s.text_color(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.change_card(&id, Edit { due: Some(0), ..Default::default() }, cx)
                            }))
                            .child(t("chattools.board.clearDue")),
                    )
                })
                .into_any_element()
        } else {
            match due_chip(&card, column, now, &p) {
                Some(chip) => chip.text_sm().into_any_element(),
                None => div().text_sm().text_color(p.muted_foreground).child("—").into_any_element(),
            }
        };

        let description = self.card_description(&card, &body, manage, seen.attach, &key, &p, cx);

        let details = inner_scroll(format!("card-details|{}", card.id), window, cx)
            .flex_none()
            .max_h(gpui_kit::relative(0.55))
            .border_b_1()
            .border_color(p.border)
            .px(px(16.0))
            .py(px(12.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(title)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(field("columns-3", t("chattools.board.column"), column_value))
                    .child(field("users", t("chattools.board.people"), people_value))
                    .child(field("tag", t("chattools.board.labels"), labels_value))
                    .child(field("calendar", t("chattools.board.due"), due_value)),
            )
            .child(description);

        // The comments: the body's thread, as the thread panel draws it.
        let open = self.threads.open.clone().filter(|o| o.id == card.id);
        let comments = open.map(|open| {
            let loading = self.core.shared.read(|s| {
                s.instance(&open.key).is_none_or(|i| {
                    i.messages
                        .get(&crate::core::threads::thread_key(&open.id))
                        .is_none_or(|l| l.loading && l.items.is_empty())
                })
            });
            self.thread_rows(&open, loading, &p, window, cx)
        });
        let reply = (!archived).then(|| self.reply_box(&p, window, cx));

        Some(
            motion::slide_in(
                div()
                    .w(px(PANEL))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(p.border)
                    .bg(p.side_surface)
                    .occlude()
                    .child(header)
                    .children(confirm)
                    .child(details)
                    .children(comments)
                    .children(reply),
                SharedString::from(format!("card-panel|{}", card.id)),
                24.0,
            )
            .into_any_element(),
        )
    }

    /// The card's description: shown like a message's text and files, or edited with them.
    #[allow(clippy::too_many_arguments)]
    fn card_description(
        &mut self,
        card: &pb::Card,
        body: &pb::Message,
        manage: bool,
        attach: bool,
        key: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let empty = body.content.trim().is_empty() && body.attachments.is_empty();
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(t("chattools.board.description").to_uppercase()),
            )
            .when(manage && !self.board.describing, |el| {
                el.child(
                    div()
                        .id("card-describe")
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.start_describing(window, cx)))
                        .child(icon("pencil").size(px(12.0)))
                        .child(if empty { t("chattools.board.addDescription") } else { t("chattools.board.edit") }),
                )
            });
        let content = if self.board.describing {
            let kept = self.board.kept.clone();
            let mut files = div().flex().flex_wrap().gap(px(6.0));
            for (n, file) in kept.iter().enumerate() {
                files = files.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .pl(px(8.0))
                        .pr(px(4.0))
                        .py(px(4.0))
                        .rounded(corner(8.0))
                        .border_1()
                        .border_color(p.border)
                        .bg(p.card)
                        .text_xs()
                        .child(crate::ui::attachments::badge(&file.filename, 20.0))
                        .child(
                            div()
                                .max_w(px(160.0))
                                .truncate()
                                .font_weight(FontWeight::BOLD)
                                .child(file.filename.clone()),
                        )
                        .when(file.spoiler, |el| {
                            el.child(icon("eye-off").size(px(12.0)).text_color(p.muted_foreground))
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("kept-x|{n}")))
                                .p(px(2.0))
                                .rounded(corner(4.0))
                                .text_color(p.muted_foreground)
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.muted).text_color(p.destructive))
                                .tooltip({
                                    let tip =
                                        t_with("chattools.board.removeFile", &[("name", Arg::Str(&file.filename))]);
                                    move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if n < this.board.kept.len() {
                                        this.board.kept.remove(n);
                                    }
                                    cx.notify();
                                }))
                                .child(icon("x").size(px(12.0))),
                        ),
                );
            }
            let uploading = self.files_state().uploading;
            // Pasted or dropped pictures and files join the description's, as the paperclip's do.
            let mut body = Textarea::new(&self.board.body).appearance(false);
            if attach {
                body = body.on_paste(self.paste_to_channel(cx));
            }
            let (drop_bg, primary) = (alpha(p.primary, 0.08), p.primary);
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .id("card-body-box")
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(alpha(p.primary, 0.6))
                        .bg(p.card)
                        .text_sm()
                        .when(attach, |el| {
                            el.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg).border_color(primary))
                                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                    cx.stop_propagation();
                                    this.add_files(paths.paths().to_vec(), cx)
                                }))
                        })
                        .child(div().ml(px(-10.0)).child(body)),
                )
                .when(!kept.is_empty(), |el| el.child(files))
                .children(self.file_tray(&p, cx))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .when(attach, |el| el.child(self.attach_button(&p, cx)))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.2))
                                .text_color(p.muted_foreground)
                                .child(t("chattools.board.markdownHint")),
                        )
                        .child(soft_button("card-body-cancel", t("chattools.board.cancel"), &p).h(px(32.0)).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.board.describing = false;
                                cx.notify();
                            }),
                        ))
                        .child(
                            primary_button("card-body-save", t("chattools.board.save"), &p)
                                .h(px(32.0))
                                .when(uploading, |el| el.opacity(0.5))
                                .on_click(cx.listener(|this, _, _, cx| this.save_description(cx))),
                        ),
                )
                .into_any_element()
        } else if empty {
            div().text_sm().text_color(p.muted_foreground).child(t("chattools.board.noDescription")).into_any_element()
        } else {
            let this = cx.entity().downgrade();
            div()
                .text_sm()
                .when(!body.content.trim().is_empty(), |el| {
                    el.child(crate::ui::text::markdown(
                        SharedString::from(format!("card-body|{}", card.id)),
                        body.content.clone(),
                    ))
                })
                .when(body.edited_at.is_some(), |el| {
                    el.child(
                        div().text_size(px(10.4)).text_color(p.muted_foreground).child(t("chattools.board.edited")),
                    )
                })
                .when(!body.attachments.is_empty(), |el| {
                    el.child(crate::ui::attachments::attachments_view(
                        &body.id,
                        &body.attachments,
                        &self.core.shared.read(|s| s.instance(key).map(|i| i.url.clone())).unwrap_or_default(),
                        &p,
                        &this,
                        key,
                        false,
                    ))
                })
                .into_any_element()
        };
        div().flex().flex_col().gap(px(6.0)).child(head).child(content).into_any_element()
    }

    /// The board's archived cards, latest first, to open or put back.
    fn archived_panel(&mut self, seen: &Seen, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        let b = &seen.board;
        let list = b.archived.clone().unwrap_or_default();
        let mut rows = div().flex().flex_col().gap(px(8.0)).p(px(12.0));
        for card in &list.cards {
            let column = b.columns.iter().find(|c| c.id == card.column_id).map_or("?".to_owned(), |c| c.name.clone());
            let (open, restore) = (card.id.clone(), card.id.clone());
            let hover = p.muted;
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(10.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .child(
                        div()
                            .id(SharedString::from(format!("archived|{}", card.id)))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| this.open_card(open.clone(), window, cx)))
                            .child(div().truncate().text_sm().font_weight(FontWeight::BOLD).child(card.title.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(format!("{column} · {}", short_day(board::ms(card.archived_at.as_ref())))),
                            ),
                    )
                    .when(seen.manage, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("restore|{}", card.id)))
                                .flex_none()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(corner(8.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.primary)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.change_card(&restore, Edit { archived: Some(false), ..Default::default() }, cx)
                                }))
                                .child(t("chattools.board.restore")),
                        )
                    }),
            );
        }
        if list.cards.is_empty() && !list.loading {
            rows = rows.child(
                div()
                    .p(px(16.0))
                    .text_center()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("chattools.board.noArchived")),
            );
        }
        if list.loading {
            rows = rows.child(div().h(px(48.0)).rounded(radius_xl()).bg(alpha(p.muted, 0.5)));
        }
        if list.more && !list.loading {
            let hover = alpha(p.primary, 0.1);
            rows = rows.child(
                div()
                    .id("archived-more")
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .text_center()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.primary)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|this, _, _, cx| this.more_archived(cx)))
                    .child(t("chattools.board.more")),
            );
        }
        let close_hover = p.muted;
        motion::slide_in(
            div()
                .w(px(PANEL))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(p.border)
                .bg(p.side_surface)
                .occlude()
                .child(
                    div()
                        .h(px(56.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(p.border)
                        .child(icon("archive").size(px(16.0)).text_color(p.muted_foreground))
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(t("chattools.board.archived")),
                        )
                        .child(
                            div()
                                .id("archived-close")
                                .px(px(12.0))
                                .py(px(4.0))
                                .rounded_full()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.muted_foreground)
                                .cursor_pointer()
                                .hover(move |s| s.bg(close_hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.board.archived = false;
                                    cx.notify();
                                }))
                                .child(t("chattools.board.close")),
                        ),
                )
                .child(inner_scroll("archived-list", window, cx).flex_1().min_h_0().child(rows)),
            "archived-panel",
            24.0,
        )
        .into_any_element()
    }
}
