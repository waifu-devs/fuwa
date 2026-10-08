//! Searching a server's messages, as the web app's `search/SearchBar.tsx` and
//! `SearchPanel.tsx` do it: a field in the channel's header with filters
//! typed or picked from suggestions (`from:`, `in:`, `has:`...), recent
//! searches kept on this computer, Ctrl+F to get there from anywhere, and the
//! results beside the chat, newest first under the channel each is in, with
//! the words found lit up. Clicking one opens its channel at that message.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Entity, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, Keystroke, ParentElement as _, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::i18n::t;
use crate::core::keybinds;
use crate::core::search::{self, FilterKey};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::keys::fuzzy;
use crate::ui::motion;
use crate::ui::text::{WIDE, ms_of, tracked, when};
use crate::ui::theme::{Palette, alpha, corner, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, icon, icon_button, pal};

/// The search field and its suggestions, and the results panel while it's open.
pub struct Search {
    pub field: Entity<InputState>,
    /// The suggestion the arrows are on.
    pub active: Option<usize>,
    /// Suggestions closed with Escape until the text changes.
    pub hushed: bool,
    pub panel: Option<Panel>,
    /// The message a result opened, which glows a moment in its channel.
    pub jumped: Option<(String, Instant)>,
    /// Bumped by each search, so its first rows rise in again.
    pub run: u64,
}

impl Search {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> Self {
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        Self { field, active: None, hushed: false, panel: None, jumped: None, run: 0 }
    }
}

/// One search's results, for the server it was made in.
pub struct Panel {
    pub key: String,
    pub server: String,
    /// What was typed for the search on screen.
    pub query: String,
    request: Option<pb::SearchMessagesRequest>,
    pub results: Vec<pb::SearchResult>,
    pub total: i64,
    pub total_at_least: bool,
    pub cursor: String,
    pub loading: bool,
    /// Loading the next page, not a new search.
    pub more: bool,
    pub error: Option<String>,
    pub indexing: bool,
    pub indexed_percent: i32,
    scroll: ScrollHandle,
}

impl Panel {
    fn new(key: &str, server: &str) -> Self {
        Self {
            key: key.to_owned(),
            server: server.to_owned(),
            query: String::new(),
            request: None,
            results: Vec::new(),
            total: 0,
            total_at_least: false,
            cursor: String::new(),
            loading: false,
            more: false,
            error: None,
            indexing: false,
            indexed_percent: 0,
            scroll: ScrollHandle::new(),
        }
    }
}

/// One row of suggestions.
#[derive(Clone)]
struct Suggestion {
    id: String,
    label: String,
    /// Shown dimmer after the label (a member's username, a date).
    hint: String,
    glyph: &'static str,
    user: Option<pb::User>,
    /// Put in place of the word at the caret.
    insert: Option<String>,
    /// Searched right away (a recent search).
    search: Option<String>,
    /// A key with its colon: the value comes next, without a space.
    open: bool,
}

struct Group {
    title: &'static str,
    items: Vec<Suggestion>,
}

fn glyph_of(key: FilterKey) -> &'static str {
    match key {
        FilterKey::From | FilterKey::Mentions => "user",
        FilterKey::In => "hash",
        FilterKey::Has => "paperclip",
        _ => "calendar",
    }
}

/// Results drawn beyond these first ones don't rise in one after another.
const STAGGER_ROWS: usize = 10;
/// How close to the end of the results the next page starts loading, in pixels.
const LOAD_AHEAD: f32 = 800.0;

impl FuwaApp {
    /// The server whose messages the field searches: the one on screen.
    fn search_place(&self) -> Option<(String, String)> {
        match &self.nav {
            Nav::Server { key, server } => Some((key.clone(), server.clone())),
            _ => None,
        }
    }

    fn search_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.search.field.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Ctrl+F: to the field, from anywhere in a server.
    pub(crate) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_place().is_none() {
            return;
        }
        self.search.hushed = false;
        self.search.field.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// The panel's gone when you leave its server; the field forgets what was typed.
    pub(crate) fn search_after_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.search_place();
        let keep =
            self.search.panel.as_ref().is_some_and(|p| here.as_ref() == Some(&(p.key.clone(), p.server.clone())));
        if !keep {
            self.search.panel = None;
            if !self.search.field.read(cx).value().is_empty() {
                self.search.field.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
    }

    pub(crate) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.panel = None;
        self.search.active = None;
        self.search.field.update(cx, |s, cx| s.set_value("", window, cx));
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn recent_searches(&self) -> (String, Vec<String>) {
        let Some((key, server)) = self.search_place() else { return (String::new(), Vec::new()) };
        let me = self.core.shared.read(|s| s.instance(&key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let Some(me) = me else { return (String::new(), Vec::new()) };
        let place = search::place(&key, &me, &server);
        let list = self.prefs.recent_searches.get(&place).cloned().unwrap_or_default();
        (place, list)
    }

    /// What the field suggests for the word at the caret.
    fn suggestions(&self, cx: &Context<Self>) -> Vec<Group> {
        let Some((key, server)) = self.search_place() else { return Vec::new() };
        let (input, caret) = {
            let s = self.search.field.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let at = search::token_at(&input, caret);
        let token = &input[at];
        if let Some((name, value)) = token.split_once(':') {
            let Some(filter) = FilterKey::of(name) else { return Vec::new() };
            let value = value.trim_start_matches(['@', '#']);
            return self.core.shared.read(|s| {
                let Some(i) = s.instance(&key) else { return Vec::new() };
                match filter {
                    FilterKey::From | FilterKey::Mentions => {
                        let mut found: Vec<(f32, &pb::Member)> = i
                            .members
                            .get(&server)
                            .into_iter()
                            .flatten()
                            .filter_map(|m| {
                                let user = m.user.as_ref()?;
                                let a = fuzzy(value, &user.username).map(|f| f.0);
                                let b = fuzzy(value, &search::member_name(m)).map(|f| f.0);
                                Some((a.into_iter().chain(b).reduce(f32::max)?, m))
                            })
                            .collect();
                        found.sort_by(|a, b| b.0.total_cmp(&a.0));
                        let items: Vec<Suggestion> = found
                            .into_iter()
                            .take(6)
                            .filter_map(|(_, m)| {
                                let user = m.user.clone()?;
                                let insert = format!("{}:{}", filter.name(), user.username);
                                Some(Suggestion {
                                    id: insert.clone(),
                                    label: search::member_name(m),
                                    hint: user.username.clone(),
                                    glyph: "user",
                                    user: Some(user),
                                    insert: Some(insert),
                                    search: None,
                                    open: false,
                                })
                            })
                            .collect();
                        let title = if filter == FilterKey::From { "From" } else { "Mentions" };
                        if items.is_empty() { Vec::new() } else { vec![Group { title, items }] }
                    }
                    FilterKey::In => {
                        let mut found: Vec<(f32, String)> = i
                            .channels
                            .get(&server)
                            .into_iter()
                            .flatten()
                            .filter(|c| search::searchable(c))
                            .filter_map(|c| Some((fuzzy(value, &c.name)?.0, c.name.clone())))
                            .collect();
                        found.sort_by(|a, b| b.0.total_cmp(&a.0));
                        let items: Vec<Suggestion> = found
                            .into_iter()
                            .take(6)
                            .map(|(_, name)| Suggestion {
                                id: format!("in:{name}"),
                                insert: Some(format!("in:{name}")),
                                label: name,
                                hint: String::new(),
                                glyph: "hash",
                                user: None,
                                search: None,
                                open: false,
                            })
                            .collect();
                        if items.is_empty() { Vec::new() } else { vec![Group { title: "In channel", items }] }
                    }
                    FilterKey::Has => {
                        let v = value.to_lowercase();
                        let items: Vec<Suggestion> = search::HAS_VALUES
                            .iter()
                            .filter(|(_, _, aliases, _)| aliases.iter().any(|a| a.starts_with(&v)))
                            .map(|(value, label, _, _)| Suggestion {
                                id: format!("has:{value}"),
                                label: (*label).to_owned(),
                                hint: String::new(),
                                glyph: "paperclip",
                                user: None,
                                insert: Some(format!("has:{value}")),
                                search: None,
                                open: false,
                            })
                            .collect();
                        if items.is_empty() { Vec::new() } else { vec![Group { title: "Has", items }] }
                    }
                    _ => {
                        let today = search::today();
                        let iso = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
                        let week = today - chrono::Days::new(7);
                        let days = [
                            ("today".to_owned(), iso(today)),
                            ("yesterday".to_owned(), today.pred_opt().map(iso).unwrap_or_default()),
                            (iso(week), "a week ago".to_owned()),
                        ];
                        let v = value.to_lowercase();
                        let items: Vec<Suggestion> = days
                            .into_iter()
                            .filter(|(d, _)| d.starts_with(&v))
                            .map(|(d, hint)| Suggestion {
                                id: format!("{}:{d}", filter.name()),
                                insert: Some(format!("{}:{d}", filter.name())),
                                label: d,
                                hint,
                                glyph: "calendar",
                                user: None,
                                search: None,
                                open: false,
                            })
                            .collect();
                        if items.is_empty() { Vec::new() } else { vec![Group { title: "Date", items }] }
                    }
                }
            });
        }
        let mut groups = Vec::new();
        let keys: Vec<Suggestion> = FilterKey::ALL
            .iter()
            .filter(|(k, _)| token.is_empty() || k.name().starts_with(&token.to_lowercase()))
            .map(|(k, hint)| Suggestion {
                id: k.name().to_owned(),
                label: format!("{}:", k.name()),
                hint: (*hint).to_owned(),
                glyph: glyph_of(*k),
                user: None,
                insert: Some(format!("{}:", k.name())),
                search: None,
                open: true,
            })
            .collect();
        if !keys.is_empty() && (!token.is_empty() || input.trim().is_empty()) {
            groups.push(Group { title: "Search options", items: keys });
        }
        let (_, recent) = self.recent_searches();
        if input.trim().is_empty() && !recent.is_empty() {
            let items = recent
                .into_iter()
                .map(|q| Suggestion {
                    id: format!("recent:{q}"),
                    label: q.clone(),
                    hint: String::new(),
                    glyph: "clock",
                    user: None,
                    insert: None,
                    search: Some(q),
                    open: false,
                })
                .collect();
            groups.push(Group { title: "Recent", items });
        }
        groups
    }

    /// The arrows, Enter, Tab and Escape while the field has focus. True when it took the key.
    pub(crate) fn search_keys(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.search_focused(window, cx) {
            return false;
        }
        let m = &key.modifiers;
        if m.control || m.alt || m.platform || m.shift {
            return false;
        }
        let flat: Vec<Suggestion> = if self.search.hushed {
            Vec::new()
        } else {
            self.suggestions(cx).into_iter().flat_map(|g| g.items).collect()
        };
        let open = !flat.is_empty();
        match key.key.as_str() {
            "down" if open => {
                self.search.active = Some(self.search.active.map_or(0, |a| (a + 1) % flat.len()));
            }
            "up" if open => {
                self.search.active = Some(match self.search.active {
                    Some(0) | None => flat.len() - 1,
                    Some(a) => a - 1,
                });
            }
            "enter" | "tab" if open && self.search.active.is_some_and(|a| a < flat.len()) => {
                let pick = flat[self.search.active.unwrap_or_default()].clone();
                self.pick_suggestion(pick, window, cx);
            }
            "enter" => {
                let query = self.search.field.read(cx).value().to_string();
                if !query.trim().is_empty() {
                    self.run_search(query, window, cx);
                    self.focus.focus(window, cx);
                }
            }
            "escape" => {
                if open {
                    self.search.hushed = true;
                    self.search.active = None;
                } else if !self.search.field.read(cx).value().is_empty() {
                    self.search.field.update(cx, |s, cx| s.set_value("", window, cx));
                } else {
                    self.close_search(window, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn pick_suggestion(&mut self, pick: Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(query) = pick.search {
            self.search.field.update(cx, |s, cx| s.set_value(query.clone(), window, cx));
            self.run_search(query, window, cx);
            self.focus.focus(window, cx);
            return;
        }
        let Some(insert) = pick.insert else { return };
        self.search.field.update(cx, |state, cx| {
            let text = state.value().to_string();
            let at = search::token_at(&text, state.cursor());
            // A key goes in as it is; a value gets a space after, unless one's there already.
            let spaced = !pick.open && !text[at.end..].starts_with(char::is_whitespace);
            state.set_selected_range(at, cx);
            state.replace(if spaced { format!("{insert} ") } else { insert }, window, cx);
            state.focus(window, cx);
        });
        self.search.active = None;
        cx.notify();
    }

    /// Searches the server on screen for what was typed, from the first page.
    pub(crate) fn run_search(&mut self, query: String, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, server)) = self.search_place() else { return };
        self.search.active = None;
        self.search.run += 1;
        let mut panel = Panel::new(&key, &server);
        panel.query = query.clone();
        let request = self
            .core
            .shared
            .read(|s| s.instance(&key).map(|i| search::request_for(i, &server, &query, search::today())));
        if matches!(request, Some(Ok(Some(_)) | Err(_))) {
            // Search and threads share the side: whichever opened last closes the other.
            self.close_thread(cx);
            self.threads.listing = None;
            self.pins = None;
        }
        match request {
            Some(Ok(Some(request))) => {
                let (place, _) = self.recent_searches();
                if !place.is_empty() {
                    self.core.set_prefs(|p| p.remember_search(&place, &query, false));
                    self.prefs = self.core.prefs();
                }
                panel.request = Some(request);
                panel.loading = true;
                self.search.panel = Some(panel);
                self.fetch_results(cx);
            }
            Some(Err(problem)) => {
                panel.error = Some(problem);
                self.search.panel = Some(panel);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The next page of the search on screen.
    fn more_results(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.search.panel else { return };
        if panel.loading || panel.cursor.is_empty() || panel.request.is_none() {
            return;
        }
        panel.loading = true;
        panel.more = true;
        self.fetch_results(cx);
        cx.notify();
    }

    fn fetch_results(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = &self.search.panel else { return };
        let Some(request) = panel.request.clone() else { return };
        let (key, cursor, run) = (panel.key.clone(), panel.cursor.clone(), self.search.run);
        let core = self.core.clone();
        let more = !cursor.is_empty();
        self.run(cx, async move { core.search_page(&key, request, &cursor).await }, move |this, result, cx| {
            // A newer search, or none, took its place.
            if this.search.run != run {
                return;
            }
            let Some(panel) = &mut this.search.panel else { return };
            panel.loading = false;
            panel.more = false;
            match result {
                Ok(page) => {
                    if more {
                        panel.results.extend(page.results);
                    } else {
                        panel.results = page.results;
                        panel.total = page.total;
                        panel.total_at_least = page.total_at_least;
                    }
                    panel.cursor = page.next_cursor;
                    panel.indexing = page.indexing;
                    panel.indexed_percent = page.indexed_percent;
                }
                Err(problem) => panel.error = Some(problem.message),
            }
            cx.notify();
        });
    }

    /// Opens a result's channel at its message, loading older ones until it's there.
    fn open_result(&mut self, message: pb::Message, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, server)) = self.search_place() else { return };
        // A reply only in its thread is found there.
        if !message.thread_id.is_empty() && !message.also_in_channel {
            self.open_channel(&key, &server, &message.channel_id, window, cx);
            self.close_search(window, cx);
            self.open_thread(message.thread_id.clone(), window, cx);
            return;
        }
        self.jump_to_message(&key, &server, message, window, cx);
    }

    /// Opens a message's channel and scrolls to it, reading back as far as it has to; it glows a moment.
    pub(crate) fn jump_to_message(
        &mut self,
        key: &str,
        server: &str,
        message: pb::Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (key, server) = (key.to_owned(), server.to_owned());
        let started = Instant::now();
        self.open_channel(&key, &server, &message.channel_id, window, cx);
        let core = self.core.clone();
        let (k, s, c, id) = (key, server, message.channel_id.clone(), message.id.clone());
        self.run(cx, async move { core.find_message(&k, &s, &c, &id).await }, move |this, found, cx| {
            if !found {
                this.toast(
                    "history",
                    "That message is too far back".into(),
                    "Scroll up in the channel to find it.".into(),
                    None,
                    None,
                    cx,
                );
                return;
            }
            crate::core::reports::timing("search.open_result", started.elapsed());
            this.search.jumped = Some((message.id.clone(), Instant::now()));
            this.sync_list(cx);
            if let Some(ix) = this.rows.iter().position(|r| r.id_str() == message.id) {
                this.scroller.update(cx, |s, cx| {
                    s.scroll_to_item(ix, cx);
                });
            }
            cx.notify();
        });
    }

    // ───────────────────────── Drawing ─────────────────────────

    /// The field in a channel's header, with its suggestions under it.
    pub(crate) fn search_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let focused = self.search_focused(window, cx);
        let typed = !self.search.field.read(cx).value().is_empty();
        let combo = keybinds::action_by_id("searchServer")
            .and_then(|a| keybinds::binding_of(a, &self.prefs.keybinds))
            .map(|c| keybinds::label(&c));
        let field = div()
            .w_full()
            .h(px(36.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .rounded_full()
            .border_1()
            .border_color(if focused { alpha(p.primary, 0.5) } else { p.border.into() })
            .bg(if focused { alpha(p.background, 1.0) } else { alpha(p.background, 0.6) })
            .child(icon("search").size(px(16.0)).text_color(if focused { p.primary } else { p.muted_foreground }))
            .child(div().flex_1().min_w_0().child(Input::new(&self.search.field).appearance(false).small()))
            .when(typed, |el| {
                el.child(motion::rise(
                    div()
                        .id("search-clear")
                        .size(px(20.0))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .hover(|s| s.bg(alpha(p.foreground, 0.08)))
                        .cursor_pointer()
                        .child(icon("x").size(px(14.0)))
                        .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
                    "search-clear-in",
                    Duration::ZERO,
                    4.0,
                ))
            })
            .when(!typed && !focused, |el| {
                el.when_some(combo, |el, combo| {
                    el.child(
                        div()
                            .flex_none()
                            .px(px(6.0))
                            .rounded(crate::ui::theme::radius_md())
                            .border_1()
                            .border_color(p.border)
                            .bg(p.muted)
                            .text_size(px(10.4))
                            .line_height(px(15.6))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(combo),
                    )
                })
            });
        // Gives way to the channel's name and marks when the header is short of room.
        let mut wrap = div()
            .relative()
            // The web's `w-44 lg:w-56`: 224px from 1024px wide.
            .w(px(if window.viewport_size().width >= px(1024.0) { 224.0 } else { 176.0 }))
            .min_w(px(120.0))
            .flex_shrink(1.0)
            .child(field);
        if focused && !self.search.hushed {
            let groups = self.suggestions(cx);
            if !groups.is_empty() {
                wrap = wrap.child(self.suggestion_list(groups, &p, cx));
            }
        }
        wrap.into_any_element()
    }

    fn suggestion_list(&self, groups: Vec<Group>, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        let mut list = div().id("search-suggestions").flex().flex_col().max_h(px(384.0)).overflow_y_scroll().p(px(6.0));
        let mut row = 0usize;
        let place = self.recent_searches().0;
        for group in groups {
            let recent = group.title == "Recent";
            let clear_place = place.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    // A group's `py-1`, and its title's `px-2 pb-1 text-[0.7rem]`.
                    .px(px(8.0))
                    .pt(px(4.0))
                    .pb(px(4.0))
                    .text_size(px(11.2))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(group.title.to_uppercase(), WIDE))
                    .when(recent, |el| {
                        el.child(
                            div()
                                .id("search-recent-clear")
                                .px(px(4.0))
                                .rounded(corner(4.0))
                                .cursor_pointer()
                                .hover(|s| s.text_color(p.foreground))
                                .child(tracked(t("chattools.search.clearRecent"), WIDE))
                                .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| window.prevent_default())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let place = clear_place.clone();
                                    this.core.set_prefs(|p| {
                                        p.recent_searches.remove(&place);
                                    });
                                    this.prefs = this.core.prefs();
                                    cx.notify();
                                })),
                        )
                    }),
            );
            for item in group.items {
                let n = row;
                row += 1;
                let lit = self.search.active == Some(n);
                let forget = item.search.clone();
                let place = place.clone();
                let pick = item.clone();
                let lead: AnyElement = match &item.user {
                    Some(user) => avatar(Some(user), 20.0, &p).into_any_element(),
                    None => icon(item.glyph).size(px(16.0)).text_color(p.muted_foreground).into_any_element(),
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("suggest|{}", item.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .group("suggestion")
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(radius_xl())
                        .text_sm()
                        .line_height(px(20.0))
                        .cursor_pointer()
                        .text_color(if lit { p.foreground.into() } else { alpha(p.foreground, 0.9) })
                        .when(lit, |el| el.bg(alpha(p.primary, 0.12)))
                        .when(!lit, |el| el.hover(|s| s.bg(alpha(p.primary, 0.08))))
                        // Picking with the mouse mustn't take focus from the field first.
                        .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.pick_suggestion(pick.clone(), window, cx)),
                        )
                        .child(lead)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_baseline()
                                .gap(px(6.0))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(px(180.0))
                                        .when(item.open || item.user.is_some(), |el| el.font_weight(FontWeight::BOLD))
                                        .truncate()
                                        .child(item.label.clone()),
                                )
                                .when(!item.hint.is_empty() && item.user.is_some(), |el| {
                                    el.child(div().text_color(p.muted_foreground).truncate().child(item.hint.clone()))
                                })
                                .when(!item.hint.is_empty() && item.user.is_none() && item.open, |el| {
                                    el.child(div().text_color(p.muted_foreground).truncate().child(item.hint.clone()))
                                }),
                        )
                        .when(!item.hint.is_empty() && item.user.is_none() && !item.open, |el| {
                            el.child(
                                div().flex_none().text_xs().text_color(p.muted_foreground).child(item.hint.clone()),
                            )
                        })
                        .when_some(forget, |el, query| {
                            el.child(
                                // Shown on hover only, as the web's.
                                icon_button(SharedString::from(format!("forget|{query}")), "x", &p)
                                    .size(px(24.0))
                                    .invisible()
                                    .group_hover("suggestion", |s| s.visible())
                                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| window.prevent_default())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let (place, query) = (place.clone(), query.clone());
                                        this.core.set_prefs(|p| p.remember_search(&place, &query, true));
                                        this.prefs = this.core.prefs();
                                        cx.notify();
                                    })),
                            )
                        }),
                );
            }
            list = list.child(div().h(px(4.0)).flex_none());
        }
        // Drawn after the messages below it, so it sits over them.
        gpui_kit::deferred(
            div().absolute().top(px(45.0)).right(px(0.0)).w(px(320.0)).child(motion::rise(
                // `rounded-2xl border bg-popover shadow-xl`.
                div()
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .text_color(p.foreground)
                    .shadow(crate::ui::settings_controls::shadow_xl())
                    .occlude()
                    .child(list),
                "search-suggestions-in",
                Duration::ZERO,
                -6.0,
            )),
        )
        .with_priority(1)
    }

    /// The results beside the chat, while a search is open.
    pub(crate) fn search_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let panel = self.search.panel.as_ref()?;
        let p = pal(cx);
        // Near the end, the next page starts loading.
        let offset = panel.scroll.offset();
        let max = panel.scroll.max_offset();
        let near_end = !panel.cursor.is_empty()
            && !panel.loading
            && panel.error.is_none()
            && !panel.results.is_empty()
            && f32::from(max.y) + f32::from(offset.y) < LOAD_AHEAD;
        if near_end && panel.results.len() >= 12 {
            self.more_results(cx);
        }
        let panel = self.search.panel.as_ref()?;
        let run = self.search.run;

        let heading: AnyElement = if panel.loading && !panel.more {
            div().text_color(p.muted_foreground).child(t("chattools.search.searching")).into_any_element()
        } else if panel.error.is_some() || panel.request.is_none() {
            div().child("Search").into_any_element()
        } else {
            let n = panel.total;
            let word = if n == 1 && !panel.total_at_least { "result" } else { "results" };
            div()
                .flex()
                .items_baseline()
                .gap(px(5.0))
                .child(motion::count_up(SharedString::from(format!("count|{run}")), n as f64, Duration::ZERO, |v| {
                    group_digits(v.round() as i64)
                }))
                .when(panel.total_at_least, |el| el.child("+"))
                .child(word)
                .into_any_element()
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_base()
                    .line_height(px(24.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(heading),
            )
            .child({
                // `size-8 rounded-full`, muted until hovered.
                let (bg, fg) = (p.muted, p.foreground);
                div()
                    .id("search-close")
                    .size(px(32.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(p.muted_foreground)
                    .hover(move |s| s.bg(bg).text_color(fg))
                    .active(|s| s.opacity(0.8))
                    .child(icon("x").size(px(16.0)))
                    .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx)))
            });

        let mut body = div()
            .id("search-results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&panel.scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .flex()
            .flex_col()
            .pb(px(12.0));
        if panel.indexing {
            body = body.child(motion::rise(
                div()
                    .mx(px(10.0))
                    .mt(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(corner(12.0))
                    .bg(p.muted)
                    .flex()
                    .gap(px(8.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(icon("hourglass").size(px(14.0)).flex_none())
                    .child(div().flex_1().min_w_0().child(format!(
                        "Older messages are still being added to search ({}%). Recent ones are all here.",
                        panel.indexed_percent
                    ))),
                "search-indexing",
                Duration::ZERO,
                -6.0,
            ));
        }
        if panel.loading && !panel.more {
            body = body.child(skeleton(5, &p));
        } else if let Some(error) = &panel.error {
            body = body.child(empty("search-x", &t("chattools.search.failed"), error, &p));
        } else if panel.results.is_empty() && panel.request.is_some() {
            body = body.child(if panel.cursor.is_empty() {
                empty("search-x", &t("chattools.search.nothingFound"), &t("chattools.search.tryOther"), &p)
            } else {
                empty("search-x", &t("chattools.search.nothingNewest"), &t("chattools.search.olderNotSearched"), &p)
            });
        }
        if !(panel.loading && !panel.more) && panel.error.is_none() {
            let look = self.core.shared.read(|s| s.instance(&panel.key).map(|i| Look::of(i, &panel.server)));
            let look = look.unwrap_or_default();
            let mut last_channel = String::new();
            for (n, result) in panel.results.iter().enumerate() {
                let Some(message) = &result.message else { continue };
                if message.channel_id != last_channel {
                    last_channel = message.channel_id.clone();
                    let name = look.channels.get(&message.channel_id).cloned().unwrap_or_else(|| "channel".into());
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(16.0))
                            .pt(px(12.0))
                            .pb(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_color(p.muted_foreground)
                            .child(icon("hash").size(px(14.0)))
                            .child(name),
                    );
                }
                body = body.child(self.result_row(result, message, n, run, &look, &p, cx));
            }
            if panel.more {
                body = body.child(skeleton(2, &p));
            }
            // On a big server one search reads only so far back; this carries on from there.
            if !panel.cursor.is_empty() && !panel.loading && panel.results.len() < 12 {
                body = body.child(
                    div().flex().justify_center().py(px(12.0)).child(
                        div()
                            .id("search-further")
                            .px(px(16.0))
                            .py(px(6.0))
                            .rounded_full()
                            .bg(p.muted)
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .cursor_pointer()
                            .hover(|s| s.bg(alpha(p.primary, 0.1)).text_color(p.primary))
                            .child(t("chattools.search.further"))
                            .on_click(cx.listener(|this, _, _, cx| this.more_results(cx))),
                    ),
                );
            }
        }
        let _ = window;
        Some(
            motion::slide_in(
                div()
                    // `surface-side w-[24rem] border-l`.
                    .w(px(384.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(p.border)
                    .bg(p.side_surface)
                    .child(header)
                    .child(body),
                "search-panel-in",
                24.0,
            )
            .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn result_row(
        &self,
        result: &pb::SearchResult,
        message: &pb::Message,
        n: usize,
        run: u64,
        look: &Look,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *p;
        let author = match &message.webhook {
            Some(w) => Some(pb::User {
                id: w.webhook_id.clone(),
                username: w.name.clone(),
                display_name: w.name.clone(),
                avatar_url: w.avatar_url.clone(),
                ..Default::default()
            }),
            None => look.users.get(&message.author_id).cloned(),
        };
        let name = match &message.webhook {
            Some(w) => w.name.clone(),
            None => look
                .names
                .get(&message.author_id)
                .cloned()
                .or_else(|| author.as_ref().map(user_name))
                .unwrap_or_else(|| "Someone".into()),
        };
        let stamp = when(ms_of(message.created_at.as_ref()));
        // The message as chat draws it (Markdown, mentions, emoji, timestamps), the words found lit up.
        let text = if message.content.trim().is_empty() {
            String::new()
        } else {
            let marked = mark_hits(&message.content, &search::byte_ranges(&message.content, &result.highlights));
            hits_as_markdown(&crate::ui::mentions::mention_links(
                &crate::ui::timestamps::timestamp_nodes(&crate::ui::text::hard_breaks(
                    &crate::ui::text::images_as_links(&marked),
                )),
                &look.mentions.with(&message.emojis),
            ))
        };
        let open = message.clone();
        let id = message.id.clone();
        let row = div()
            .id(SharedString::from(format!("result|{id}")))
            .group("result")
            .relative()
            .flex()
            .gap(px(12.0))
            .mx(px(8.0))
            .mb(px(6.0))
            .px(px(12.0))
            .py(px(10.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .bg(alpha(p.card, 0.6))
            .cursor_pointer()
            .hover(|s| s.bg(p.card).border_color(p.border))
            .on_click(cx.listener(move |this, _, window, cx| this.open_result(open.clone(), window, cx)))
            .child(avatar(author.as_ref(), 32.0, &p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.2))
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(stamp),
                            ),
                    )
                    .when(!text.is_empty(), |el| {
                        // `line-clamp-6 text-sm`.
                        el.child(
                            div().text_sm().line_height(px(20.0)).max_h(px(120.0)).overflow_hidden().child(
                                gpui_kit::base::TextView::markdown(
                                    SharedString::from(format!("result-md|{run}|{}", message.id)),
                                    text,
                                )
                                .markdown_extensions(markdown_extensions())
                                .style(crate::ui::chat::chat_markdown(&p))
                                .on_link_click(|url, _, _, cx| {
                                    if !url.starts_with("fuwa:") {
                                        crate::ui::text::open_link(url, cx)
                                    }
                                })
                                .w_full(),
                            ),
                        )
                    })
                    .when(!message.attachments.is_empty(), |el| {
                        el.child(div().mt(px(4.0)).flex().flex_wrap().gap(px(4.0)).children(
                            message.attachments.iter().map(|a| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .max_w_full()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(corner(8.0))
                                    .bg(p.muted)
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(icon("paperclip").size(px(12.0)).flex_none())
                                    .child(
                                        div().truncate().child(crate::core::attachments::short_name(&a.filename, 32)),
                                    )
                            }),
                        ))
                    })
                    .when(message.content.is_empty() && !message.embeds.is_empty(), |el| {
                        let e = &message.embeds[0];
                        let line = if e.title.is_empty() { e.description.clone() } else { e.title.clone() };
                        el.child(div().text_xs().text_color(p.muted_foreground).truncate().child(line))
                    }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(p.primary)
                    .text_color(p.primary_foreground)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .invisible()
                    .group_hover("result", |s| s.visible())
                    .child(t("chattools.search.jump"))
                    .child(icon("arrow-right").size(px(12.0))),
            );
        if n < STAGGER_ROWS {
            motion::rise(
                div().child(row),
                SharedString::from(format!("result-in|{run}|{id}")),
                Duration::from_millis(35 * n as u64),
                10.0,
            )
            .into_any_element()
        } else {
            row.into_any_element()
        }
    }
}

/// What a server's results need to show who wrote them and where.
#[derive(Default)]
struct Look {
    /// Members' names here, by user id.
    names: std::collections::HashMap<String, String>,
    users: std::collections::HashMap<String, pb::User>,
    channels: std::collections::HashMap<String, String>,
    /// How chat draws mentions, roles and emoji here.
    mentions: crate::ui::mentions::Look,
}

impl Look {
    fn of(i: &crate::core::store::InstanceState, server: &str) -> Self {
        let mut look = Self { users: i.users.clone(), ..Default::default() };
        for m in i.members.get(server).into_iter().flatten() {
            if let Some(user) = &m.user {
                look.names.insert(user.id.clone(), search::member_name(m));
                look.users.insert(user.id.clone(), user.clone());
            }
        }
        for c in i.channels.get(server).into_iter().flatten() {
            look.channels.insert(c.id.clone(), c.name.clone());
        }
        look.mentions = crate::ui::mentions::Look::of(i, server);
        look
    }
}

/// Where a match starts and ends while the text goes through chat's steps (the web's `MARK_OPEN`, `MARK_CLOSE`).
const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

/// Marks the matches in a message's text (the web's `markRanges`); [`hits_as_markdown`]
/// turns the marks into what [`HitPlugin`] draws lit up (`remarkSearchHits`).
/// A match inside code, a link, a mention or any other token is left as it
/// is, so marking never changes what's shown or where a link goes.
fn mark_hits(text: &str, hits: &[Range<usize>]) -> String {
    let mut out = String::with_capacity(text.len() + hits.len() * 16);
    let mut at = 0;
    let mut hits: Vec<&Range<usize>> = hits.iter().filter(|r| r.start < r.end && r.end <= text.len()).collect();
    hits.sort_by_key(|r| r.start);
    for r in hits {
        if r.start < at || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
            continue;
        }
        let word = &text[r.clone()];
        // The whole word it's in, out to the spaces either side.
        let from = text[..r.start].rfind(char::is_whitespace).map_or(0, |i| i + 1);
        let to = text[r.end..].find(char::is_whitespace).map_or(text.len(), |i| r.end + i);
        let around = &text[from..to];
        let in_code = text[..r.start].matches('`').count() % 2 == 1;
        let risky = in_code
            || around.contains("://")
            || around.contains(['<', '>', '@', '`', '[', ']', '(', ')', '\\', '|', '!', '#'])
            || word.contains(['*', '_', '~', ':']);
        if risky {
            continue;
        }
        out.push_str(&text[at..r.start]);
        out.push(OPEN);
        out.push_str(word);
        out.push(CLOSE);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The marks [`mark_hits`] made, as `![word](fuwa-hit:)`, once chat's other steps are done.
fn hits_as_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len_utf8()..];
        let Some(end) = after.find(CLOSE) else {
            rest = after;
            continue;
        };
        out.push_str("![");
        out.push_str(&after[..end]);
        out.push_str("](fuwa-hit:)");
        rest = &after[end + CLOSE.len_utf8()..];
    }
    out.push_str(&rest.replace(CLOSE, ""));
    out
}

/// Chat's Markdown plugins and the search matches.
fn markdown_extensions() -> gpui_kit::component::text::MarkdownExtensions {
    crate::ui::emoji::markdown_extensions().plugin(HitPlugin).parser_revision(4)
}

/// A match in a result: `mark.search-hit`, the primary at 26% behind the text as it was.
struct HitPlugin;

impl gpui_kit::component::text::MarkdownPlugin for HitPlugin {
    fn name(&self) -> &str {
        "fuwa-hit"
    }

    fn parse(
        &self,
        node: &gpui_kit::component::text::markdown_ast::Node,
        _: &gpui_kit::component::text::MarkdownParseContext<'_>,
    ) -> Option<gpui_kit::component::text::MarkdownNode> {
        let gpui_kit::component::text::markdown_ast::Node::Image(image) = node else { return None };
        if image.url != "fuwa-hit:" {
            return None;
        }
        let text = image.alt.clone();
        Some(
            gpui_kit::component::text::MarkdownNode::new("fuwa-hit", SharedString::from(text.clone()))
                .text(text.clone())
                .markdown(text),
        )
    }

    fn render_inline(
        &self,
        node: &gpui_kit::component::text::MarkdownNode,
        _: &gpui_kit::component::text::InlineRenderContext,
        _: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> Option<gpui_kit::component::text::InlineElement> {
        let text = node.data::<SharedString>()?.clone();
        let p = pal(cx);
        Some(gpui_kit::component::text::InlineElement::new(
            div().px(px(1.4)).rounded(px(3.5)).bg(alpha(p.primary, 0.26)).child(text).into_any_element(),
        ))
    }
}

/// 12345 → "12,345".
fn group_digits(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (k, c) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Rows that pulse while results are on their way.
pub(crate) fn skeleton(rows: usize, p: &Palette) -> impl IntoElement {
    let shade = alpha(p.muted_foreground, 0.12);
    let faint = alpha(p.muted_foreground, 0.05);
    div().flex().flex_col().gap(px(8.0)).px(px(10.0)).py(px(10.0)).children((0..rows).map(move |n| {
        div()
            .flex()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(12.0))
            .rounded(corner(16.0))
            .bg(faint)
            .child(div().size(px(32.0)).flex_none().rounded_full().bg(shade))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(div().h(px(12.0)).w(px(110.0)).rounded_full().bg(shade))
                    .child(div().h(px(12.0)).w(px(220.0 - 20.0 * (n % 3) as f32)).rounded_full().bg(shade)),
            )
            .with_animation(
                SharedString::from(format!("search-skeleton-{n}")),
                Animation::new(Duration::from_millis(1400)).repeat(),
                move |el, t| {
                    let wave = ((t + n as f32 * 0.12) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                    el.opacity(0.45 + 0.4 * wave)
                },
            )
    }))
}

/// What the list shows when there's nothing to list.
pub(crate) fn empty(glyph: &'static str, title: &str, text: &str, p: &Palette) -> impl IntoElement + use<> {
    motion::rise(
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .px(px(24.0))
            .py(px(48.0))
            .child(
                div()
                    .size(px(48.0))
                    .rounded(corner(16.0))
                    .bg(p.muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .child(icon(glyph).size(px(24.0))),
            )
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
            .child(
                div().max_w(px(240.0)).text_sm().text_center().text_color(p.muted_foreground).child(text.to_owned()),
            ),
        SharedString::from(format!("search-empty|{title}")),
        Duration::ZERO,
        8.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_marked_only_where_marking_changes_nothing_else() {
        let text = "the cake is `the lie` at https://the.example and @the <@&01J0> **the**";
        let hits: Vec<Range<usize>> = text.match_indices("the").map(|(i, w)| i..i + w.len()).collect();
        let marked = hits_as_markdown(&mark_hits(text, &hits));
        assert!(marked.starts_with("![the](fuwa-hit:) cake is `the lie` at https://the.example and @the"));
        assert!(marked.ends_with("**![the](fuwa-hit:)**"));
        assert_eq!(group_digits(10000), "10,000");
        assert_eq!(group_digits(999), "999");
    }
}
