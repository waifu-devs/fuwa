//! The emoji picker over the composer, like the web app's `EmojiPicker`:
//! recently used ones, the server's own, your other servers' on the same
//! instance (each under its icon), then the standard set by category, with
//! skin tones. Search matches names and the words people use for them. Only
//! the rows on screen are drawn, so the whole set scrolls smoothly; arrows
//! move through them from the search box and Enter picks.

use crate::ui::emoji::InColor as _;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::Focusable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, radians,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::emoji_glyph;
use crate::ui::emoji::{self, Catalog, Choice};
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{card, icon, server_icon};

/// Emoji in a row, how tall each row is, and each section's title (the web's grid).
const COLS: usize = 8;
const CELL: f32 = 40.0;
const HEADER: f32 = 30.0;
/// The grid's own height (the web's `h-72`).
const GRID: f32 = 288.0;
/// Recently used emoji shown, at most.
const RECENT_ROWS: usize = 2;

const GROUP_ICONS: [(&str, &str); 8] = [
    ("people", "face-slightly-smiling"),
    ("nature", "leaf"),
    ("food", "pizza"),
    ("activities", "trophy"),
    ("travel", "plane"),
    ("objects", "lightbulb"),
    ("symbols", "heart"),
    ("flags", "flag"),
];
const TONES: [&str; 6] = ["✋\u{fe0f}", "✋🏻", "✋🏼", "✋🏽", "✋🏾", "✋🏿"];
const TONE_NAMES: [&str; 6] = [
    "chattools.emoji.tone.default",
    "chattools.emoji.tone.light",
    "chattools.emoji.tone.mediumLight",
    "chattools.emoji.tone.medium",
    "chattools.emoji.tone.mediumDark",
    "chattools.emoji.tone.dark",
];

/// What a section's header shows next to its title.
#[derive(Clone)]
pub enum Mark {
    Icon(&'static str),
    Server(Rc<pb::Server>),
}

/// A row of the grid: a section's header, or up to `COLS` emoji.
#[derive(Clone)]
pub enum PickRow {
    Header { id: String, title: String, mark: Mark },
    Cells { start: usize },
}

/// The grid laid out: its rows, every emoji in order, and where each
/// section starts (for the buttons along the top).
#[derive(Default)]
pub struct PickLayout {
    pub rows: Vec<PickRow>,
    pub flat: Vec<Choice>,
    /// The row each emoji is in.
    pub cell_row: Vec<usize>,
    /// Each section's id, header row and mark.
    pub sections: Vec<(String, usize, Mark)>,
    /// Where each row starts, from the grid's top.
    pub tops: Vec<f32>,
    pub searching: bool,
}

impl PickLayout {
    fn build(sections: Vec<(String, String, Mark, Vec<Choice>)>, searching: bool) -> Self {
        let mut out = PickLayout { searching, ..Default::default() };
        for (id, title, mark, choices) in sections {
            if choices.is_empty() {
                continue;
            }
            out.sections.push((id.clone(), out.rows.len(), mark.clone()));
            out.tops.push(out.height());
            out.rows.push(PickRow::Header { id, title, mark });
            for chunk in choices.chunks(COLS) {
                let row = out.rows.len();
                out.tops.push(out.height());
                out.rows.push(PickRow::Cells { start: out.flat.len() });
                for choice in chunk {
                    out.flat.push(choice.clone());
                    out.cell_row.push(row);
                }
            }
        }
        out
    }

    /// How tall the rows so far are.
    fn height(&self) -> f32 {
        match (self.tops.last(), self.rows.last()) {
            (Some(top), Some(PickRow::Header { .. })) => top + HEADER,
            (Some(top), Some(PickRow::Cells { .. })) => top + CELL,
            _ => 0.0,
        }
    }

    /// The emoji in a row of cells.
    fn cells(&self, row: usize) -> std::ops::Range<usize> {
        let PickRow::Cells { start } = self.rows[row] else { return 0..0 };
        let end = self.rows[row + 1..]
            .iter()
            .find_map(|r| match r {
                PickRow::Cells { start } => Some(*start),
                PickRow::Header { .. } => None,
            })
            .unwrap_or(self.flat.len())
            .min(start + COLS);
        start..end
    }

    /// A section's title.
    fn title_of(&self, section: &str) -> &str {
        self.rows
            .iter()
            .find_map(|r| match r {
                PickRow::Header { id, title, .. } if id == section => Some(title.as_str()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The section a row is in.
    fn section_at(&self, row: usize) -> Option<&str> {
        self.sections.iter().rev().find(|(_, top, _)| *top <= row).map(|(id, _, _)| id.as_str())
    }
}

/// The picker's state, kept on the app while it's open.
pub struct EmojiPicker {
    /// The emoji lit by the pointer or the arrows, in `PickLayout::flat`.
    pub active: Option<usize>,
    /// The arrows lit it, not the pointer (which poses it by hovering).
    pub by_keys: bool,
    pub tones_open: bool,
    /// The grid's rows: titles are shorter than rows of emoji, so it's a list of its own.
    pub scroll: gpui_kit::ListState,
    /// Recently used, as they were when it opened, so picking a few in a row
    /// doesn't shift the grid under the pointer.
    pub recent: Vec<String>,
    /// The layout and what it was made from.
    pub layout: Option<(u64, Rc<PickLayout>)>,
}

impl Default for EmojiPicker {
    fn default() -> Self {
        Self {
            active: None,
            by_keys: false,
            tones_open: false,
            scroll: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, px(200.0)),
            recent: Vec::new(),
            layout: None,
        }
    }
}

impl EmojiPicker {
    /// Back to the top (a new search starts there).
    pub fn to_top(&self) {
        self.scroll.scroll_to(gpui_kit::ListOffset::default());
    }
}

impl FuwaApp {
    /// The smiley in the composer that opens the picker.
    pub(crate) fn emoji_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.emoji_open;
        let fg = p.primary;
        // A composer tool (`widgets::tool_button`) that also tilts and grows when pointed
        // at, and shrinks when pressed: the web's `whileHover={{ scale: 1.12, rotate: -10 }}`
        // and `whileTap={{ scale: 0.85 }}`.
        div()
            .id("emoji-open")
            .size(px(36.0))
            .mb(px(2.0))
            .flex_none()
            .rounded(crate::ui::theme::radius_xl())
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(if open { p.primary } else { p.muted_foreground })
            .when(open, |el| el.bg(alpha(p.primary, 0.1)))
            .hover(move |s| s.text_color(fg).scale(1.12).rotate(radians((-10f32).to_radians())))
            .active(|s| s.scale(0.85))
            .child(icon("face-slightly-smiling").size(px(18.0)))
            // It toggles as it's pressed: a press while it's open has already closed it
            // (the panel's press outside), so it mustn't open it again on release.
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    if open {
                        this.close_emoji(window, cx);
                    } else {
                        this.open_emoji(window, cx);
                    }
                }),
            )
            .into_any_element()
    }

    pub(crate) fn open_emoji(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_gifs(cx);
        self.emoji_open = true;
        self.picker = None;
        self.emoji = EmojiPicker { recent: self.core.prefs().recent_emoji, ..Default::default() };
        self.emoji_born = Some(std::time::Instant::now());
        self.emoji_query.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn close_emoji(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        self.emoji.layout = None;
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// The catalog for the open place: a server's emoji and your other
    /// servers', or none in direct messages.
    fn emoji_catalog(&self) -> Catalog {
        match self.target() {
            Some(Target::Channel { key, server, .. }) => self
                .core
                .shared
                .read(|s| s.instance(&key).map(|i| Catalog::of(&i.servers, &i.emojis, &server)).unwrap_or_default()),
            _ => Catalog::default(),
        }
    }

    /// The grid for what's typed, kept until the query, the emoji or the tone change.
    fn emoji_layout(&mut self, query: &str, tone: u8) -> Rc<PickLayout> {
        use std::hash::{Hash as _, Hasher as _};
        let catalog = self.emoji_catalog();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (query, tone).hash(&mut h);
        for c in &catalog.emojis {
            (&c.emoji.id, &c.emoji.url, &c.alias, c.here).hash(&mut h);
        }
        catalog.servers.iter().for_each(|s| (&s.id, &s.name).hash(&mut h));
        let digest = h.finish();
        if let Some((was, layout)) = &self.emoji.layout
            && *was == digest
        {
            return layout.clone();
        }
        let servers: Vec<pb::Server> = match self.target() {
            Some(Target::Channel { key, .. }) => {
                self.core.shared.read(|s| s.instance(&key).map(|i| i.servers.clone()).unwrap_or_default())
            }
            _ => Vec::new(),
        };
        let searching = !query.trim().trim_matches(':').is_empty();
        let sections = if searching {
            let found = emoji::search(query, &catalog, tone, usize::MAX);
            let title = if found.is_empty() {
                t("chattools.emoji.nothingFound")
            } else {
                t_with("chattools.emoji.found", &[("count", Arg::Num(found.len() as i64))])
            };
            vec![("results".to_owned(), title, Mark::Icon("search"), found)]
        } else {
            let recent: Vec<Choice> = self
                .emoji
                .recent
                .iter()
                .filter_map(|key| Choice::recalled(key, &catalog, tone))
                .take(COLS * RECENT_ROWS)
                .collect();
            let mut sections = vec![("recent".to_owned(), t("chattools.emoji.recent"), Mark::Icon("clock"), recent)];
            for (server, list) in &catalog.sections {
                let s = &catalog.servers[*server];
                let full = servers.iter().find(|x| x.id == s.id).cloned().unwrap_or_else(|| pb::Server {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    icon_url: s.icon_url.clone(),
                    ..Default::default()
                });
                let choices = list.iter().map(|&n| Choice::custom(&catalog, &catalog.emojis[n])).collect();
                sections.push((format!("s:{}", s.id), s.name.clone(), Mark::Server(Rc::new(full)), choices));
            }
            for group in emoji::standard() {
                let mark =
                    GROUP_ICONS.iter().find(|(id, _)| *id == group.id).map_or("face-slightly-smiling", |(_, i)| i);
                let choices = group.emojis.iter().map(|e| Choice::standard(e, tone)).collect();
                // The standard set's categories by id; the data's own English name stands in for any other.
                let key = format!("chattools.emoji.group.{}", group.id);
                let title =
                    if GROUP_ICONS.iter().any(|(id, _)| *id == group.id) { t(&key) } else { group.name.clone() };
                sections.push((group.id.clone(), title, Mark::Icon(mark), choices));
            }
            sections
        };
        let layout = Rc::new(PickLayout::build(sections, searching));
        if self.emoji.scroll.item_count() != layout.rows.len() {
            self.emoji.scroll.reset(layout.rows.len());
        }
        self.emoji.layout = Some((digest, layout.clone()));
        layout
    }

    /// Puts a picked emoji in the box and remembers it. The picker stays
    /// open, so a few can go in one after another.
    fn choose_emoji(&mut self, choice: &Choice, window: &mut Window, cx: &mut Context<Self>) {
        let key = choice.key.clone();
        self.core.set_prefs(|prefs| {
            prefs.recent_emoji.retain(|k| *k != key);
            prefs.recent_emoji.insert(0, key);
            prefs.recent_emoji.truncate(crate::core::config::MAX_RECENT_EMOJI);
        });
        let used = match (&choice.url, &choice.from) {
            (Some(_), Some(_)) => "emoji.pick.other_server",
            (Some(_), None) => "emoji.pick.server",
            (None, _) => "emoji.pick.standard",
        };
        crate::core::reports::used(used);
        self.composer.update(cx, |state, cx| state.replace(format!("{} ", choice.insert), window, cx));
        self.emoji_query.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// Arrow keys and Enter from the search box. True when it took the key.
    pub(crate) fn emoji_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, layout)) = self.emoji.layout.clone() else { return false };
        if layout.flat.is_empty() {
            return false;
        }
        let last = layout.flat.len() - 1;
        let at_end = {
            let state = self.emoji_query.read(cx);
            state.cursor() >= state.value().len()
        };
        let next = match (key, self.emoji.active) {
            ("enter", active) => {
                let choice = layout.flat[active.unwrap_or(0)].clone();
                self.choose_emoji(&choice, window, cx);
                return true;
            }
            ("right", None) if at_end => 0,
            ("right", Some(n)) if at_end => (n + 1).min(last),
            ("left", Some(n)) if at_end => n.saturating_sub(1),
            ("down", None) => 0,
            ("down" | "up", Some(n)) => {
                let row = layout.cell_row[n];
                let col = n - layout.cells(row).start;
                let rows: Box<dyn Iterator<Item = usize>> =
                    if key == "down" { Box::new(row + 1..layout.rows.len()) } else { Box::new((0..row).rev()) };
                let to = rows.map(|r| layout.cells(r)).find(|cells| !cells.is_empty());
                match to {
                    Some(cells) => (cells.start + col).min(cells.end - 1),
                    None => n,
                }
            }
            _ => return false,
        };
        self.emoji.active = Some(next);
        self.emoji.by_keys = true;
        self.emoji.scroll.scroll_to_reveal_item(layout.cell_row[next]);
        cx.notify();
        true
    }

    /// The picker itself, floating above the composer's emoji button (the
    /// web's `top-end` placement: its right edge on the button's, 8px above it).
    /// Once closed, it's drawn a moment more on its way out.
    pub(crate) fn emoji_panel(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.emoji_open;
        let going = motion::kept("emoji-panel", open.then_some(&()), window, cx).map(|(_, t)| t);
        if !open && going.is_none() {
            return None;
        }
        let query = self.emoji_query.read(cx).value().to_string();
        let tone = self.core.prefs().skin_tone;
        let layout = self.emoji_layout(&query, tone);
        if layout.searching && self.emoji.active.is_none() && !layout.flat.is_empty() {
            self.emoji.active = Some(0);
        }
        let active = self.emoji.active.filter(|n| *n < layout.flat.len());
        let scrolled = self.emoji.scroll.logical_scroll_top();
        let top_row = scrolled.item_ix;
        let current = layout.section_at(top_row).map(str::to_owned);
        // Springs keyed to this opening, so the next one starts in place.
        let opened = format!("{:?}", self.emoji_born);
        let fresh_open = self.emoji_born.is_some_and(|t| t.elapsed() < Duration::from_millis(450));

        let focused = self.emoji_query.read(cx).focus_handle(cx).is_focused(window);
        let search = div()
            .p(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex_1()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .pl(px(10.0))
                    .rounded(crate::ui::theme::radius_xl())
                    // Opaque, so the focus ring (a shadow) stays outside it as on the web.
                    .bg(crate::ui::theme::mix(p.card, p.muted, 0.6))
                    .when(focused, |el| {
                        el.shadow(vec![gpui_kit::BoxShadow {
                            color: alpha(p.primary, 0.4),
                            offset: gpui_kit::point(px(0.0), px(0.0)),
                            blur_radius: px(0.0),
                            spread_radius: px(2.0),
                            inset: false,
                        }])
                    })
                    .child(icon("search").size(px(16.0)).text_color(p.muted_foreground))
                    // The web's text starts 32px in; the field keeps some padding of its own.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .ml(px(-6.0))
                            .child(Input::new(&self.emoji_query).appearance(false).text_sm()),
                    ),
            )
            .child(self.tone_button(tone, p, cx));

        let rail = (!layout.searching).then(|| {
            let mut rail =
                div().relative().flex().px(px(8.0)).py(px(6.0)).gap(px(2.0)).border_b_1().border_color(p.border);
            // The section in view's fill glides along the rail (the web's `layoutId`).
            if let Some(at) = layout.sections.iter().position(|(id, _, _)| current.as_deref() == Some(id.as_str())) {
                let x = motion::follow(
                    SharedString::from(format!("emoji-rail-lit|{opened}")),
                    at as f32 * 34.0,
                    window,
                    cx,
                );
                rail = rail.child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .left(px(8.0 + x))
                        .size(px(32.0))
                        .rounded(crate::ui::theme::radius_lg())
                        .bg(alpha(p.primary, 0.12)),
                );
            }
            for (id, row, mark) in &layout.sections {
                let lit = current.as_deref() == Some(id.as_str());
                let row = *row;
                let face = match mark {
                    Mark::Icon(name) => icon(name).size(px(16.0)).into_any_element(),
                    Mark::Server(server) => server_icon(server, 20.0, 6.0, p).into_any_element(),
                };
                let title = layout.title_of(id).to_owned();
                let fg = p.foreground;
                rail = rail.child(
                    div()
                        .id(SharedString::from(format!("emoji-rail|{id}")))
                        .size(px(32.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(crate::ui::theme::radius_lg())
                        .cursor_pointer()
                        .text_color(if lit { p.primary } else { p.muted_foreground })
                        .when(!lit, |el| el.hover(move |s| s.text_color(fg)))
                        .tooltip(move |window, cx| crate::ui::overlay::Tip::new(title.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emoji.scroll.scroll_to(gpui_kit::ListOffset { item_ix: row, offset_in_item: px(0.0) });
                            cx.notify();
                        }))
                        .child(face),
                );
            }
            rail
        });

        let rows_layout = layout.clone();
        let fresh = !layout.searching && fresh_open;
        let by_keys = self.emoji.by_keys;
        let grid = gpui_kit::list(
            self.emoji.scroll.clone(),
            cx.processor(move |this, row: usize, _window, cx| {
                let p = crate::ui::widgets::pal(cx);
                this.emoji_row(&rows_layout, row, active, by_keys, fresh, &p, cx)
            }),
        )
        .size_full();
        // The lit emoji's fill glides from cell to cell under the list (the web's
        // spring-driven `bg-primary/12` square), kept where the grid has scrolled to.
        let lit = active.map(|n| {
            let row = layout.cell_row[n];
            let x = (n - layout.cells(row).start) as f32 * CELL;
            let x = motion::follow(SharedString::from(format!("emoji-lit-x|{opened}")), x, window, cx);
            let y = motion::follow(SharedString::from(format!("emoji-lit-y|{opened}")), layout.tops[row], window, cx);
            let scrolled =
                layout.tops.get(scrolled.item_ix).copied().unwrap_or(0.0) + f32::from(scrolled.offset_in_item);
            div()
                .absolute()
                .left(px(x))
                .top(px(y - scrolled))
                .size(px(CELL))
                .rounded(corner(10.0))
                .bg(alpha(p.primary, 0.12))
        });
        // The web's grid keeps 8px at the sides and below; the list takes no padding of its own.
        let grid = div()
            .h(px(GRID))
            .px(px(8.0))
            .pb(px(8.0))
            .child(div().relative().size_full().overflow_hidden().children(lit).child(grid));
        // The section in view stays named at the top as the grid scrolls (the web's sticky title),
        // the next one dropping in as it takes over.
        let sticky = current.as_ref().and_then(|id| {
            let (_, _, mark) = layout.sections.iter().find(|(s, _, _)| s == id)?;
            let title = div().bg(p.card).px(px(12.0)).child(section_title(layout.title_of(id), mark, p));
            let title: AnyElement = if fresh_open {
                title.into_any_element()
            } else {
                motion::rise(title, SharedString::from(format!("emoji-sticky|{id}")), Duration::ZERO, -6.0)
                    .into_any_element()
            };
            Some(div().absolute().top_0().left_0().right_0().child(title))
        });
        let grid = if layout.flat.is_empty() {
            div()
                .h(px(GRID))
                .flex()
                .flex_col()
                .items_center()
                .pt(px(40.0))
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t_with("chattools.emoji.noneCalled", &[("query", Arg::Str(query.trim().trim_matches(':')))]))
                .into_any_element()
        } else {
            div().relative().child(grid).children(sticky).into_any_element()
        };

        let shown = active.and_then(|n| layout.flat.get(n));
        let body = div()
            .w(px(352.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(crate::ui::theme::radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(vec![gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.25),
                offset: gpui_kit::point(px(0.0), px(25.0)),
                blur_radius: px(50.0),
                spread_radius: px(-12.0),
                inset: false,
            }])
            .child(search)
            .when_some(rail, |el, rail| el.child(rail))
            .child(grid)
            .child(preview(shown, p, fresh_open));
        let panel = div()
            .id("emoji-panel")
            .absolute()
            .right(px(self.tool_right(crate::ui::composer::Tool::Emoji)))
            .bottom(gpui_kit::relative(1.0))
            .when(open, |el| el.on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_emoji(window, cx))))
            // The web's `scale: 0.92, y: 8`, out of its corner over the button.
            .child(motion::pop_in(body.mb(px(-1.0)), "emoji-panel-in", (1.0, 1.0), 0.92, 8.0))
            // And out: `opacity: 0, scale: 0.95, y: 6`.
            .when_some(going, |el, t| {
                crate::ui::chat::closing(
                    el,
                    t,
                    crate::ui::chat::Gone { scale: 0.95, x: 0.0, y: 6.0, origin: (1.0, 1.0) },
                )
            })
            .into_any_element();
        Some(panel)
    }

    /// One row: a section's title, or its emoji.
    #[allow(clippy::too_many_arguments)]
    fn emoji_row(
        &self,
        layout: &PickLayout,
        row: usize,
        active: Option<usize>,
        by_keys: bool,
        fresh: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &layout.rows[row] {
            PickRow::Header { id, title, mark } => div()
                .id(SharedString::from(format!("emoji-head|{id}")))
                .px(px(4.0))
                .child(section_title(title, mark, p))
                .into_any_element(),
            PickRow::Cells { .. } => {
                let mut cells = div().id(SharedString::from(format!("emoji-row|{row}"))).h(px(CELL)).flex();
                for n in layout.cells(row) {
                    let posed = by_keys && active == Some(n);
                    cells = cells.child(self.emoji_cell(&layout.flat[n], n, posed, fresh, cx));
                }
                cells.into_any_element()
            }
        }
    }

    /// One emoji. Pointed at (or lit by the arrows, `posed`) it grows and
    /// tilts, and shrinks while held (the web's `.emoji-cell`); its fill is
    /// the one gliding under the grid.
    fn emoji_cell(&self, choice: &Choice, n: usize, posed: bool, fresh: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("emoji|{n}|{}", choice.key));
        let pick = choice.clone();
        let glyph = div().child(emoji_glyph(choice, 30.0));
        // They pop in one after another as the picker opens (the web's `emoji-in`).
        let glyph = if fresh {
            motion::pop(
                glyph,
                SharedString::from(format!("emoji-in|{n}")),
                0.6,
                0.0,
                Duration::from_millis(8 * n.min(40) as u64),
            )
            .into_any_element()
        } else {
            glyph.into_any_element()
        };
        let tilt = radians((-6f32).to_radians());
        div()
            .id(id)
            .size(px(CELL))
            .flex()
            .items_center()
            .justify_center()
            .rounded(corner(10.0))
            .cursor_pointer()
            .when(posed, |el| el.scale(1.22).rotate(tilt))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && (this.emoji.active != Some(n) || this.emoji.by_keys) {
                    this.emoji.active = Some(n);
                    this.emoji.by_keys = false;
                    cx.notify();
                }
            }))
            .hover(move |s| s.scale(1.22).rotate(tilt))
            .active(|s| s.scale(0.88).rotate(radians(0.0)))
            .on_click(cx.listener(move |this, _, window, cx| this.choose_emoji(&pick, window, cx)))
            .child(glyph)
            .into_any_element()
    }

    /// The hand that sets the skin tone, opening a row of the six to pick from.
    fn tone_button(&self, tone: u8, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.emoji.tones_open;
        let button = div()
            .id("emoji-tone")
            .size(px(36.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(corner(10.0))
            .cursor_pointer()
            .in_color()
            .text_size(px(19.0))
            // The web's `hover:bg-primary/10`, `whileHover={{ scale: 1.12, rotate: -8 }}` and `whileTap={{ scale: 0.88 }}`.
            .hover({
                let bg = alpha(p.primary, 0.1);
                move |s| s.bg(bg).scale(1.12).rotate(radians((-8f32).to_radians()))
            })
            .active(|s| s.scale(0.88))
            .when(open, |el| el.bg(alpha(p.primary, 0.12)))
            .tooltip(move |window, cx| {
                crate::ui::overlay::Tip::new(t_with(
                    "chattools.emoji.skinToneIs",
                    &[("tone", Arg::Str(&t(TONE_NAMES[tone as usize])))],
                ))
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.emoji.tones_open = !this.emoji.tones_open;
                cx.notify();
            }))
            .child(TONES[tone as usize]);
        let mut wrap = div().relative().child(button);
        if open {
            let mut row = card(p).p(px(4.0)).flex().gap(px(2.0)).rounded(corner(12.0));
            for (k, hand) in TONES.iter().enumerate() {
                let n = k as u8;
                let hand = div()
                    .id(SharedString::from(format!("emoji-tone|{n}")))
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(8.0))
                    .cursor_pointer()
                    .in_color()
                    .text_size(px(18.0))
                    .when(n == tone, |el| el.bg(alpha(p.primary, 0.15)))
                    .hover(|s| s.scale(1.2))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.core.set_prefs(|prefs| prefs.skin_tone = n);
                        this.emoji.tones_open = false;
                        cx.notify();
                    }))
                    .child(*hand);
                // Each hand pops in a moment after the one before it.
                row = row.child(motion::pop(
                    hand,
                    SharedString::from(format!("emoji-tone-in|{n}")),
                    0.5,
                    0.0,
                    Duration::from_millis(25 * k as u64),
                ));
            }
            // It drops out of the hand's corner (the web's `scale: 0.85, y: -4`).
            wrap = wrap.child(div().absolute().top(px(40.0)).right_0().child(motion::pop_in(
                row,
                "emoji-tones-in",
                (1.0, 0.0),
                0.85,
                -4.0,
            )));
        }
        wrap.into_any_element()
    }
}

/// A section's title: its icon (or server's picture) and name, small and spaced out.
fn section_title(title: &str, mark: &Mark, p: &Palette) -> impl IntoElement {
    div()
        .h(px(HEADER))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(10.4))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(match mark {
            Mark::Icon(name) => icon(name).size(px(12.0)).into_any_element(),
            Mark::Server(server) => server_icon(server, 16.0, 6.0, p).into_any_element(),
        })
        .child(tracked(title.to_uppercase(), WIDE))
}

/// The emoji you're on, big, with the name to type and where it's from,
/// each one rising in as it's lit (the web's `AnimatePresence` keyed by it).
fn preview(shown: Option<&Choice>, p: &Palette, fresh: bool) -> AnyElement {
    let base = div()
        .h(px(48.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .border_t_1()
        .border_color(p.border)
        .text_sm();
    let Some(choice) = shown else {
        return base.text_color(p.muted_foreground).child(t("chattools.emoji.hint")).into_any_element();
    };
    let base = div().flex().items_center().gap(px(10.0)).min_w_0();
    let from = match (&choice.from, &choice.url) {
        (Some(server), _) => t_with("chattools.emoji.fromServer", &[("server", Arg::Str(server))]),
        (None, Some(_)) => t("chattools.emoji.fromHere"),
        (None, None) => String::new(),
    };
    let base = base
        .child(div().size(px(32.0)).flex_none().flex().items_center().justify_center().child(emoji_glyph(choice, 28.0)))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::BOLD).truncate().child(format!(":{}:", choice.name)))
                .when(!from.is_empty(), |el| {
                    el.child(div().text_xs().text_color(p.muted_foreground).truncate().child(from))
                }),
        );
    let shown: AnyElement = if fresh {
        base.into_any_element()
    } else {
        motion::rise(base, SharedString::from(format!("emoji-preview|{}", choice.key)), Duration::ZERO, 6.0)
            .into_any_element()
    };
    div()
        .h(px(48.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .border_t_1()
        .border_color(p.border)
        .text_sm()
        .child(shown)
        .into_any_element()
}
