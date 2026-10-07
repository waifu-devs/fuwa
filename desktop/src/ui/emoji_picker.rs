//! The emoji picker over the composer, like the web app's `EmojiPicker`:
//! recently used ones, the server's own, your other servers' on the same
//! instance (each under its icon), then the standard set by category, with
//! skin tones. Search matches names and the words people use for them. Only
//! the rows on screen are drawn, so the whole set scrolls smoothly; arrows
//! move through them from the search box and Enter picks.

use crate::ui::emoji::InColor as _;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, ScrollStrategy,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, uniform_list,
};

use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::emoji_glyph;
use crate::ui::emoji::{self, Catalog, Choice};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{card, icon, server_icon};

/// Emoji in a row, and how tall each row (and header) is.
const COLS: usize = 8;
const CELL: f32 = 40.0;
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
const TONES: [&str; 6] = ["✋", "✋🏻", "✋🏼", "✋🏽", "✋🏾", "✋🏿"];
const TONE_NAMES: [&str; 6] = ["Default", "Light", "Medium-light", "Medium", "Medium-dark", "Dark"];

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
            out.rows.push(PickRow::Header { id, title, mark });
            for chunk in choices.chunks(COLS) {
                let row = out.rows.len();
                out.rows.push(PickRow::Cells { start: out.flat.len() });
                for choice in chunk {
                    out.flat.push(choice.clone());
                    out.cell_row.push(row);
                }
            }
        }
        out
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

    /// The section a row is in.
    fn section_at(&self, row: usize) -> Option<&str> {
        self.sections.iter().rev().find(|(_, top, _)| *top <= row).map(|(id, _, _)| id.as_str())
    }
}

/// The picker's state, kept on the app while it's open.
#[derive(Default)]
pub struct EmojiPicker {
    /// The emoji lit by the pointer or the arrows, in `PickLayout::flat`.
    pub active: Option<usize>,
    pub tones_open: bool,
    pub scroll: gpui_kit::UniformListScrollHandle,
    /// Recently used, as they were when it opened, so picking a few in a row
    /// doesn't shift the grid under the pointer.
    pub recent: Vec<String>,
    /// The layout and what it was made from.
    pub layout: Option<(u64, Rc<PickLayout>)>,
}

impl FuwaApp {
    /// The smiley in the composer that opens the picker.
    pub(crate) fn emoji_button(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.emoji_open;
        crate::ui::widgets::tool_button("emoji-open", "face-slightly-smiling", open, p)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.emoji_open {
                    this.close_emoji(window, cx);
                } else {
                    this.open_emoji(window, cx);
                }
            }))
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
            let title = if found.is_empty() { "Nothing found".to_owned() } else { format!("{} found", found.len()) };
            vec![("results".to_owned(), title, Mark::Icon("search"), found)]
        } else {
            let recent: Vec<Choice> = self
                .emoji
                .recent
                .iter()
                .filter_map(|key| Choice::recalled(key, &catalog, tone))
                .take(COLS * RECENT_ROWS)
                .collect();
            let mut sections = vec![("recent".to_owned(), "Recently used".to_owned(), Mark::Icon("clock"), recent)];
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
                sections.push((group.id.clone(), group.name.clone(), Mark::Icon(mark), choices));
            }
            sections
        };
        let layout = Rc::new(PickLayout::build(sections, searching));
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
        self.emoji.scroll.scroll_to_item(layout.cell_row[next], ScrollStrategy::Nearest);
        cx.notify();
        true
    }

    /// The picker itself, floating over the composer's right end.
    pub(crate) fn emoji_panel(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.emoji_query.read(cx).value().to_string();
        let tone = self.core.prefs().skin_tone;
        let layout = self.emoji_layout(&query, tone);
        if layout.searching && self.emoji.active.is_none() && !layout.flat.is_empty() {
            self.emoji.active = Some(0);
        }
        let active = self.emoji.active.filter(|n| *n < layout.flat.len());
        let top_row = {
            let offset = self.emoji.scroll.0.borrow().base_handle.offset();
            ((-f32::from(offset.y)) / CELL).max(0.0) as usize
        };
        let current = layout.section_at(top_row).map(str::to_owned);

        let search = div()
            .p(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(p.border)
            .child(div().flex_1().child(Input::new(&self.emoji_query).prefix(icon("search").size(px(16.0)))))
            .child(self.tone_button(tone, p, cx));

        let rail = (!layout.searching).then(|| {
            let mut rail = div().flex().px(px(6.0)).py(px(4.0)).gap(px(2.0)).border_b_1().border_color(p.border);
            for (id, row, mark) in &layout.sections {
                let lit = current.as_deref() == Some(id.as_str());
                let row = *row;
                let face = match mark {
                    Mark::Icon(name) => icon(name).size(px(15.0)).into_any_element(),
                    Mark::Server(server) => server_icon(server, 16.0, 5.0, p).into_any_element(),
                };
                rail = rail.child(
                    div()
                        .id(SharedString::from(format!("emoji-rail|{id}")))
                        .flex_1()
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(8.0))
                        .cursor_pointer()
                        .text_color(if lit { p.primary } else { p.muted_foreground })
                        .when(lit, |el| el.bg(alpha(p.primary, 0.12)))
                        .hover(|s| s.bg(alpha(p.primary, 0.08)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emoji.scroll.scroll_to_item(row, ScrollStrategy::Top);
                            cx.notify();
                        }))
                        .child(face),
                );
            }
            rail
        });

        let rows_layout = layout.clone();
        let fresh = !layout.searching && self.emoji_born.is_some_and(|t| t.elapsed() < Duration::from_millis(450));
        let grid = uniform_list(
            "emoji-rows",
            layout.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let p = crate::ui::widgets::pal(cx);
                range.map(|row| this.emoji_row(&rows_layout, row, active, fresh, &p, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.emoji.scroll)
        .h(px(288.0))
        .px(px(8.0));
        let grid = if layout.flat.is_empty() {
            div()
                .h(px(288.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_color(p.muted_foreground)
                .child(icon("search-x").size(px(28.0)))
                .child(div().text_sm().child(format!("No emoji called “{}”.", query.trim().trim_matches(':'))))
                .into_any_element()
        } else {
            grid.into_any_element()
        };

        let shown = active.and_then(|n| layout.flat.get(n));
        let body = card(p)
            .w(px(352.0))
            .rounded(corner(18.0))
            .overflow_hidden()
            .child(search)
            .when_some(rail, |el, rail| el.child(rail))
            .child(grid)
            .child(preview(shown, p));
        div()
            .id("emoji-panel")
            .absolute()
            .right(px(20.0))
            .bottom(gpui_kit::relative(1.0))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_emoji(window, cx)))
            .child(motion::rise(body.mb(px(-8.0)), "emoji-panel-rise", Duration::ZERO, 12.0))
            .into_any_element()
    }

    /// One row: a section's title, or its emoji.
    fn emoji_row(
        &self,
        layout: &PickLayout,
        row: usize,
        active: Option<usize>,
        fresh: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &layout.rows[row] {
            PickRow::Header { id, title, mark } => div()
                .id(SharedString::from(format!("emoji-head|{id}")))
                .h(px(CELL))
                .px(px(4.0))
                .pb(px(4.0))
                .flex()
                .items_end()
                .gap(px(6.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(match mark {
                    Mark::Icon(name) => icon(name).size(px(12.0)).into_any_element(),
                    Mark::Server(server) => server_icon(server, 16.0, 5.0, p).into_any_element(),
                })
                .child(div().min_w_0().truncate().child(title.to_uppercase()))
                .into_any_element(),
            PickRow::Cells { .. } => {
                let mut cells = div().id(SharedString::from(format!("emoji-row|{row}"))).h(px(CELL)).flex();
                for n in layout.cells(row) {
                    cells = cells.child(self.emoji_cell(&layout.flat[n], n, active == Some(n), fresh, p, cx));
                }
                cells.into_any_element()
            }
        }
    }

    fn emoji_cell(
        &self,
        choice: &Choice,
        n: usize,
        lit: bool,
        fresh: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = SharedString::from(format!("emoji|{n}|{}", choice.key));
        let pick = choice.clone();
        let glyph = div().child(emoji_glyph(choice, 26.0));
        let glyph = if fresh {
            motion::rise(
                glyph,
                SharedString::from(format!("emoji-in|{n}")),
                Duration::from_millis(8 * n.min(40) as u64),
                6.0,
            )
            .into_any_element()
        } else {
            glyph.into_any_element()
        };
        div()
            .id(id)
            .size(px(CELL))
            .flex()
            .items_center()
            .justify_center()
            .rounded(corner(10.0))
            .cursor_pointer()
            .when(lit, |el| el.bg(alpha(p.primary, 0.12)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.emoji.active != Some(n) {
                    this.emoji.active = Some(n);
                    cx.notify();
                }
            }))
            .active(|s| s.top(px(1.0)))
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
            .hover(|s| s.bg(alpha(p.primary, 0.08)))
            .when(open, |el| el.bg(alpha(p.primary, 0.12)))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!("Skin tone: {}", TONE_NAMES[tone as usize]))
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
            for (n, hand) in TONES.iter().enumerate() {
                let n = n as u8;
                row = row.child(
                    div()
                        .id(SharedString::from(format!("emoji-tone|{n}")))
                        .size(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(8.0))
                        .cursor_pointer()
                        .in_color()
                        .text_size(px(18.0))
                        .when(n == tone, |el| el.bg(alpha(p.primary, 0.14)))
                        .hover(|s| s.bg(alpha(p.primary, 0.08)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.set_prefs(|prefs| prefs.skin_tone = n);
                            this.emoji.tones_open = false;
                            cx.notify();
                        }))
                        .child(*hand),
                );
            }
            // Rising moves it by its own offset, so it sits in an absolute box of its own.
            wrap = wrap.child(div().absolute().top(px(40.0)).right_0().child(motion::rise(
                row,
                "emoji-tones-rise",
                Duration::ZERO,
                6.0,
            )));
        }
        wrap.into_any_element()
    }
}

/// The emoji you're on, big, with the name to type and where it's from.
fn preview(shown: Option<&Choice>, p: &Palette) -> AnyElement {
    let base = div().h(px(52.0)).px(px(12.0)).flex().items_center().gap(px(10.0)).border_t_1().border_color(p.border);
    let Some(choice) = shown else {
        return base
            .text_sm()
            .text_color(p.muted_foreground)
            .child("Pick an emoji, or type to find one")
            .into_any_element();
    };
    let from = match (&choice.from, &choice.url) {
        (Some(server), _) => format!("From {server}"),
        (None, Some(_)) => "From this server".to_owned(),
        (None, None) => String::new(),
    };
    base.child(emoji_glyph(choice, 30.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_sm().font_weight(FontWeight::BOLD).truncate().child(format!(":{}:", choice.name)))
                .when(!from.is_empty(), |el| {
                    el.child(div().text_xs().text_color(p.muted_foreground).truncate().child(from))
                }),
        )
        .into_any_element()
}
