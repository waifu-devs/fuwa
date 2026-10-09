//! GIFs, as the web app's `GifPicker`, `GifPanel` and `GifMessage`: the GIF
//! button by the composer and the picker it opens (search through the
//! instance, moods to browse, your saved GIFs and uploads, and what you sent
//! lately on this computer), and GIFs in messages with a star to save them.
//! Picking one sends it at once. Every picture comes through the instance, so
//! the provider never learns who looked. With reduced motion, GIFs hold still.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement as _, ObjectFit, ParentElement as _, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled, StyledImage as _, Subscription, Task, WeakEntity, Window, div, img, px, radians,
};

use crate::core::gifs::{self as gifs, KeptGif};
use crate::pb;
use crate::ui::app::{FuwaApp, Target};
use crate::ui::chat::Gone;
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, corner, radius_xl};
use crate::ui::widgets::{card, icon};

/// The picker's size, and its grid's spacing.
const PANEL_W: f32 = 384.0;
/// Inside the card's 1px border, which the grids lay out in.
const INNER_W: f32 = PANEL_W - 2.0;
const BODY_H: f32 = 336.0;
const GAP: f32 = 6.0;
const PAD: f32 = 8.0;
/// Tiles drawn beyond what's in view, and how near the end the next page loads.
const OVERSCAN: f32 = 480.0;
const LOAD_AHEAD: f32 = 600.0;
/// How long typing rests before it's searched.
const PAUSE: Duration = Duration::from_millis(300);
/// The biggest GIF of your own that's read to upload: the most this app draws a
/// picture from (`ui/http.rs`). The instance's own cap, if it has one, still decides.
const MAX_UPLOAD: u64 = 10 * 1024 * 1024;
/// How long whether GIFs are on is believed.
const SETTINGS_FOR: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Browse,
    Saved,
    Recent,
}

/// What picking a tile does: store a search result first, or send a sealed GIF as it is.
#[derive(Clone)]
enum Pick {
    Result(String),
    Gif(pb::MessageGif),
}

/// One tile in a grid.
#[derive(Clone)]
struct Tile {
    key: String,
    url: String,
    width: i32,
    height: i32,
    title: String,
    pick: Pick,
    /// Its stored link, when it has one, to save or unsave it by.
    stored: Option<String>,
}

impl Tile {
    fn of_result(r: &pb::GifResult) -> Self {
        Self {
            key: r.id.clone(),
            url: r.preview_url.clone(),
            width: r.width,
            height: r.height,
            title: r.title.clone(),
            pick: Pick::Result(r.id.clone()),
            stored: None,
        }
    }

    fn of_gif(g: &pb::MessageGif) -> Self {
        Self {
            key: g.url.clone(),
            url: g.url.clone(),
            width: g.width,
            height: g.height,
            title: g.title.clone(),
            pick: Pick::Gif(g.clone()),
            stored: Some(g.url.clone()),
        }
    }
}

/// Search results (or trending), a page at a time.
struct Results {
    key: String,
    query: String,
    tiles: Vec<Tile>,
    /// The next page's cursor; `None` once there's no more.
    next: Option<String>,
    busy: bool,
    failed: Option<String>,
    run: u64,
}

/// The picker and what it knows, kept on the app.
pub struct Gifs {
    pub open: bool,
    pub query: Entity<InputState>,
    tab: Tab,
    /// Trending, browsed from its tile.
    trending: bool,
    typing: Option<Task<()>>,
    results: Option<Results>,
    run: u64,
    /// Per instance: whether search is on, whose it is, and when that was asked.
    settings: HashMap<String, (bool, i32, Instant)>,
    asking: HashSet<String>,
    /// Per instance: your saved GIFs, once they've come.
    saved: HashMap<String, Vec<pb::SavedGif>>,
    categories: HashMap<String, Result<Vec<pb::GifCategory>, String>>,
    /// The tile being sent.
    sending: Option<String>,
    uploading: bool,
    scroll: ScrollHandle,
    born: Option<Instant>,
}

impl Gifs {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search GIFs"));
        let subs = vec![cx.subscribe_in(&query, window, |this: &mut FuwaApp, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                this.gifs.trending = false;
                this.search_gifs_soon(cx);
            }
        })];
        let gifs = Self {
            open: false,
            query,
            tab: Tab::default(),
            trending: false,
            typing: None,
            results: None,
            run: 0,
            settings: HashMap::new(),
            asking: HashSet::new(),
            saved: HashMap::new(),
            categories: HashMap::new(),
            sending: None,
            uploading: false,
            scroll: ScrollHandle::new(),
            born: None,
        };
        (gifs, subs)
    }

    /// The links of your saved GIFs at an instance, for the stars on messages.
    pub fn saved_urls(&self, key: &str) -> HashSet<String> {
        self.saved.get(key).into_iter().flatten().filter_map(|s| s.gif.as_ref().map(|g| g.url.clone())).collect()
    }
}

impl FuwaApp {
    /// Where a GIF would go: a server's own channel you may attach files in.
    pub(crate) fn gif_place(&self) -> Option<(String, String, String)> {
        if !self.can_attach() {
            return None;
        }
        match self.target()? {
            Target::Channel { key, server, channel } => Some((key, server, channel)),
            _ => None,
        }
    }

    /// Asks an instance whether GIFs are on, now and then, and your saved ones the first time.
    pub(crate) fn ask_gifs(&mut self, key: &str, cx: &mut Context<Self>) {
        let fresh = self.gifs.settings.get(key).is_some_and(|(_, _, at)| at.elapsed() < SETTINGS_FOR);
        if fresh || !self.gifs.asking.insert(key.to_owned()) {
            return;
        }
        let first = !self.gifs.saved.contains_key(key);
        let (core, k) = (self.core.clone(), key.to_owned());
        self.run(
            cx,
            async move {
                let (on, provider) = core.gif_settings(&k).await;
                let saved = if first { Some(core.saved_gifs(&k).await) } else { None };
                (k, on, provider, saved)
            },
            |this, (key, on, provider, saved), cx| {
                this.gifs.asking.remove(&key);
                this.gifs.settings.insert(key.clone(), (on, provider, Instant::now()));
                if let Some(saved) = saved {
                    // Not knowing them only leaves the stars empty.
                    this.gifs.saved.insert(key, saved.unwrap_or_default());
                }
                cx.notify();
            },
        );
    }

    /// The provider to credit, where GIF search is on at the open place.
    pub(crate) fn gifs_on(&self, key: &str) -> Option<i32> {
        self.gifs.settings.get(key).and_then(|(on, provider, _)| on.then_some(*provider))
    }

    /// The GIF button by the composer, where GIF search is on and you may attach files.
    pub(crate) fn gif_button(&mut self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (key, _, _) = self.gif_place()?;
        self.ask_gifs(&key, cx);
        self.gifs_on(&key)?;
        let open = self.gifs.open;
        let (fg, rest) = (p.primary, if open { p.primary } else { p.muted_foreground });
        // The web's GIF button: a little "GIF" tag in the tool's `size-9 rounded-xl`,
        // which tilts and grows when pointed at and shrinks when pressed.
        let tag = div()
            .id("gif-open-tag")
            .px(px(4.0))
            .rounded(corner(6.0))
            .border_2()
            .border_color(rest)
            .group_hover("gif-open", move |s| s.border_color(fg))
            .text_size(px(9.6))
            .line_height(px(15.2))
            .font_weight(FontWeight::BLACK)
            .child(tracked("GIF", WIDE));
        Some(
            div()
                .id("gif-open")
                .group("gif-open")
                .size(px(36.0))
                .mb(px(2.0))
                .flex_none()
                .rounded(radius_xl())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(rest)
                .when(open, |el| el.bg(alpha(p.primary, 0.1)))
                .hover(move |s| s.text_color(fg).scale(1.1).rotate(radians(6f32.to_radians())))
                .active(|s| s.scale(0.85))
                .child(tag)
                .tooltip(|window, cx| crate::ui::overlay::Tip::new("GIFs").build(window, cx))
                // It toggles as it's pressed: a press while it's open has already closed it
                // (the panel's press outside), so it mustn't open it again on release.
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if open {
                            this.close_gifs(cx);
                        } else {
                            this.open_gifs(window, cx);
                        }
                    }),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn open_gifs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.emoji_open {
            self.close_emoji(window, cx);
        }
        let g = &mut self.gifs;
        g.open = true;
        g.tab = Tab::Browse;
        g.trending = false;
        g.results = None;
        g.sending = None;
        g.born = Some(Instant::now());
        g.scroll = ScrollHandle::new();
        let place = self.gif_place();
        let credit =
            place.as_ref().map_or("", |(key, _, _)| gifs::provider_name(self.gifs_on(key).unwrap_or_default()));
        let placeholder = if credit.is_empty() { "Search GIFs".to_owned() } else { format!("Search {credit}") };
        self.gifs.query.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        if let Some((key, _, _)) = place {
            self.load_categories(&key, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_gifs(&mut self, cx: &mut Context<Self>) {
        let g = &mut self.gifs;
        g.open = false;
        g.typing = None;
        // What it showed stays for its way out; opening starts afresh.
        cx.notify();
    }

    fn load_categories(&mut self, key: &str, cx: &mut Context<Self>) {
        if matches!(self.gifs.categories.get(key), Some(Ok(_))) {
            return;
        }
        self.gifs.categories.remove(key);
        let (core, k) = (self.core.clone(), key.to_owned());
        self.run(
            cx,
            async move {
                let result = core.gif_categories(&k).await.map_err(|e| e.message);
                (k, result)
            },
            |this, (key, result), cx| {
                this.gifs.categories.insert(key, result);
                cx.notify();
            },
        );
    }

    /// Searches a moment after typing rests; clearing the box goes back.
    fn search_gifs_soon(&mut self, cx: &mut Context<Self>) {
        self.gifs.typing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            let _ = this.update(cx, |this, cx| {
                let typed: String = this.gifs.query.read(cx).value().trim().chars().take(gifs::QUERY).collect();
                if typed.is_empty() {
                    if !this.gifs.trending {
                        this.gifs.results = None;
                    }
                    cx.notify();
                } else if this.gifs.results.as_ref().is_none_or(|r| r.query != typed) {
                    this.start_gif_search(typed, cx);
                }
            });
        }));
    }

    fn start_gif_search(&mut self, query: String, cx: &mut Context<Self>) {
        let Some((key, _, _)) = self.gif_place() else { return };
        self.gifs.run += 1;
        self.gifs.scroll = ScrollHandle::new();
        self.gifs.results = Some(Results {
            key,
            query,
            tiles: Vec::new(),
            next: Some(String::new()),
            busy: false,
            failed: None,
            run: self.gifs.run,
        });
        self.more_gifs(cx);
    }

    /// The next page of results.
    fn more_gifs(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.gifs.results.as_mut() else { return };
        let Some(cursor) = r.next.clone() else { return };
        if r.busy {
            return;
        }
        r.busy = true;
        let (core, key, query, run) = (self.core.clone(), r.key.clone(), r.query.clone(), r.run);
        self.run(cx, async move { core.search_gifs(&key, &query, &cursor).await }, move |this, result, cx| {
            let Some(r) = this.gifs.results.as_mut().filter(|r| r.run == run) else { return };
            r.busy = false;
            match result {
                Ok(page) => {
                    let seen: HashSet<String> = r.tiles.iter().map(|t| t.key.clone()).collect();
                    r.tiles.extend(page.results.iter().filter(|x| !seen.contains(&x.id)).map(Tile::of_result));
                    r.next = (!page.next_cursor.is_empty()).then_some(page.next_cursor);
                }
                Err(err) => {
                    r.failed = Some(err.message);
                    r.next = None;
                }
            }
            cx.notify();
        });
    }

    fn browse_trending(&mut self, cx: &mut Context<Self>) {
        self.gifs.trending = true;
        self.start_gif_search(String::new(), cx);
        cx.notify();
    }

    fn browse_category(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.gifs.query.update(cx, |state, cx| state.set_value(query.clone(), window, cx));
        self.gifs.typing = None;
        self.start_gif_search(query, cx);
    }

    fn gifs_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gifs.trending = false;
        self.gifs.results = None;
        self.gifs.typing = None;
        self.gifs.query.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Sends a tile: a search result is stored on the instance first.
    fn pick_gif(&mut self, tile: Tile, cx: &mut Context<Self>) {
        let Some((key, server, channel)) = self.gif_place() else { return };
        if self.gifs.sending.is_some() {
            return;
        }
        self.gifs.sending = Some(tile.key.clone());
        let core = self.core.clone();
        self.run(
            cx,
            async move {
                let gif = match tile.pick {
                    Pick::Gif(gif) => gif,
                    Pick::Result(id) => core.prepare_gif(&key, pb::prepare_gif_request::From::ResultId(id)).await?,
                };
                core.send_gif(&key, &server, &channel, gif).await
            },
            |this, result, cx| {
                this.gifs.sending = None;
                match result {
                    Ok(()) => this.close_gifs(cx),
                    Err(err) => {
                        this.toast("circle-alert", "Couldn't send that GIF".into(), err.message, None, None, cx)
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Saves a tile's GIF, or takes it off your saved ones.
    fn toggle_tile(&mut self, tile: Tile, cx: &mut Context<Self>) {
        let Some((key, _, _)) = self.gif_place() else { return };
        match (tile.stored, tile.pick) {
            (Some(url), _) => self.toggle_saved_gif(key, url, cx),
            (None, Pick::Result(id)) => {
                let core = self.core.clone();
                let k = key.clone();
                self.run(
                    cx,
                    async move { core.prepare_gif(&k, pb::prepare_gif_request::From::ResultId(id)).await },
                    move |this, result, cx| match result {
                        Ok(gif) => this.toggle_saved_gif(key, gif.url, cx),
                        Err(err) => {
                            this.toast("circle-alert", "Couldn't save that GIF".into(), err.message, None, None, cx)
                        }
                    },
                );
            }
            (None, Pick::Gif(_)) => {}
        }
    }

    /// Saves a GIF by its link, or takes it off at once (putting it back if that fails).
    pub(crate) fn toggle_saved_gif(&mut self, key: String, url: String, cx: &mut Context<Self>) {
        let list = self.gifs.saved.entry(key.clone()).or_default();
        let core = self.core.clone();
        if let Some(n) = list.iter().position(|s| s.gif.as_ref().is_some_and(|g| g.url == url)) {
            let removed = list.remove(n);
            let k = key.clone();
            self.run(cx, async move { core.unsave_gif(&k, &url).await }, move |this, result, cx| {
                if let Err(err) = result {
                    let list = this.gifs.saved.entry(key).or_default();
                    list.insert(n.min(list.len()), removed);
                    this.toast("circle-alert", "Couldn't remove that GIF".into(), err.message, None, None, cx);
                }
                cx.notify();
            });
        } else {
            let k = key.clone();
            self.run(cx, async move { core.save_gif(&k, &url).await }, move |this, result, cx| match result {
                Ok(Some(saved)) => {
                    let list = this.gifs.saved.entry(key).or_default();
                    let url = saved.gif.as_ref().map(|g| g.url.clone()).unwrap_or_default();
                    list.retain(|s| s.gif.as_ref().is_none_or(|g| g.url != url));
                    list.insert(0, saved);
                    cx.notify();
                }
                Ok(None) => {}
                Err(err) => this.toast("circle-alert", "Couldn't save that GIF".into(), err.message, None, None, cx),
            });
        }
        cx.notify();
    }

    fn pick_gif_file(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Upload".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.upload_gif(path, cx));
        })
        .detach();
    }

    /// Uploads a GIF of your own to your saved ones.
    fn upload_gif(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some((key, _, _)) = self.gif_place() else { return };
        self.gifs.uploading = true;
        let core = self.core.clone();
        let k = key.clone();
        self.run(
            cx,
            async move {
                let unreadable = |_: std::io::Error| {
                    crate::core::api::Problem::new(tonic::Code::NotFound, "Couldn't read that file.")
                };
                // Too big is said before reading it all.
                if tokio::fs::metadata(&path).await.map_err(unreadable)?.len() > MAX_UPLOAD {
                    return Err(crate::core::api::Problem::new(
                        tonic::Code::InvalidArgument,
                        "That GIF is over 10 MB.",
                    ));
                }
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|_| crate::core::api::Problem::new(tonic::Code::NotFound, "Couldn't read that file."))?;
                if !bytes.starts_with(b"GIF8") {
                    return Err(crate::core::api::Problem::new(tonic::Code::InvalidArgument, "Pick a GIF file."));
                }
                let url = core.upload_picture(&k, pb::MediaPurpose::Gif, "image/gif", bytes).await?;
                core.save_gif(&k, &url).await
            },
            move |this, result, cx| {
                this.gifs.uploading = false;
                match result {
                    Ok(saved) => {
                        if let Some(saved) = saved {
                            this.gifs.saved.entry(key).or_default().insert(0, saved);
                        }
                        this.toast("star", "Uploaded to your GIFs".into(), String::new(), None, None, cx);
                    }
                    Err(err) => this.toast("circle-alert", "Couldn't upload that".into(), err.message, None, None, cx),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    // ───────────────────────── The picker ─────────────────────────

    /// The picker, floating over the composer's right end.
    pub(crate) fn gif_panel(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let place = self.gif_place();
        if place.is_none() {
            self.gifs.open = false;
        }
        // Once closed, it's drawn a moment more on its way out.
        let open = self.gifs.open;
        let going = motion::kept("gif-panel", open.then_some(&()), window, cx).map(|(_, t)| t);
        if !open && going.is_none() {
            return None;
        }
        let (key, _, _) = place?;
        let credit = gifs::provider_name(self.gifs_on(&key).unwrap_or_default());
        let searching = self.gifs.results.is_some();
        let fresh = self.gifs.born.is_some_and(|t| t.elapsed() < Duration::from_millis(450));

        // Back takes the glass's place while searching, each popping in as the other goes.
        let lead: AnyElement = if searching {
            let (bg, fg) = (p.muted, p.foreground);
            let back = div()
                .id("gif-back")
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(crate::ui::theme::radius_lg())
                .cursor_pointer()
                .text_color(p.muted_foreground)
                .hover(move |s| s.bg(bg).text_color(fg))
                .on_click(cx.listener(|this, _, window, cx| this.gifs_back(window, cx)))
                .child(icon("arrow-left").size(px(16.0)));
            motion::pop(back, "gif-back-in", 0.6, 45.0, Duration::ZERO).into_any_element()
        } else if fresh {
            icon("search").size(px(16.0)).into_any_element()
        } else {
            motion::pop(div().child(icon("search").size(px(16.0))), "gif-glass-in", 0.6, 0.0, Duration::ZERO)
                .into_any_element()
        };
        let search = div()
            .p(px(8.0))
            .border_b_1()
            .border_color(p.border)
            .child(Input::new(&self.gifs.query).prefix(lead).cleanable(true));

        let tabs = (!searching).then(|| self.gif_tabs(&key, p, cx));
        // What each tab or search shows slides in as it takes over.
        let shown = match self.gifs.results.as_ref() {
            Some(r) => format!("results|{}", r.run),
            None => format!("tab|{}", self.gifs.tab as u8),
        };
        let body = match self.gifs.results.as_ref() {
            Some(_) => self.gif_results(&key, p, window, cx),
            None => match self.gifs.tab {
                Tab::Browse => self.gif_browse(&key, p, window, cx),
                Tab::Saved => self.gif_saved(&key, p, window, cx),
                Tab::Recent => {
                    let tiles: Vec<Tile> = self
                        .core
                        .prefs()
                        .recent_gifs
                        .get(&key)
                        .into_iter()
                        .flatten()
                        .map(|g: &KeptGif| Tile::of_gif(&g.gif()))
                        .collect();
                    if tiles.is_empty() {
                        empty("clock", "Nothing sent yet", "GIFs you send show up here, on this computer.", p)
                    } else {
                        self.gif_grid("recent", &key, tiles, false, false, p, window, cx)
                    }
                }
            },
        };
        let footer = (!credit.is_empty()).then(|| {
            div()
                .h(px(28.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .justify_end()
                .border_t_1()
                .border_color(p.border)
                .text_size(px(10.5))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(tracked(format!("Powered by {credit} · through this instance"), WIDE))
        });
        let panel = card(p)
            .w(px(PANEL_W))
            .rounded(corner(18.0))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(search)
            .children(tabs)
            .child(div().h(px(BODY_H)).relative().child(if fresh {
                body
            } else {
                motion::slide_in(div().size_full().child(body), SharedString::from(format!("gif-body|{shown}")), 12.0)
                    .into_any_element()
            }))
            .children(footer);
        Some(
            div()
                .id("gif-panel")
                .absolute()
                .right(px(20.0))
                .bottom(gpui_kit::relative(1.0))
                .when(open, |el| el.on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_gifs(cx))))
                // The web's `scale: 0.92, y: 10` from the bottom right, over the button.
                .child(motion::pop_in(panel.mb(px(-8.0)), "gif-panel-in", (1.0, 1.0), 0.92, 10.0))
                // And out: `opacity: 0, scale: 0.95, y: 8`.
                .when_some(going, |el, t| {
                    el.map(|el| {
                        crate::ui::chat::closing(el, t, Gone { scale: 0.95, x: 0.0, y: 8.0, origin: (1.0, 1.0) })
                    })
                })
                .into_any_element(),
        )
    }

    fn gif_tabs(&self, key: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let count = self.gifs.saved.get(key).map_or(0, Vec::len);
        let saved = if count > 0 { format!("Saved · {count}") } else { "Saved".to_owned() };
        let mut row = div().relative().flex().gap(px(4.0)).px(px(8.0)).pt(px(8.0));
        // The open tab's fill glides between them (the web's `layoutId="gif-tab"`).
        let w = (INNER_W - PAD * 2.0 - 8.0) / 3.0;
        row = row.child(crate::ui::motion::glide(
            div().absolute().top(px(8.0)).w(px(w)).h(px(32.0)).rounded(corner(9.0)).bg(alpha(p.primary, 0.1)),
            format!("gif-tab|{:?}", self.gifs.born),
            PAD + self.gifs.tab as u8 as f32 * (w + 4.0),
            cx,
            |el, x| el.left(px(x)),
        ));
        for (tab, label, glyph) in [
            (Tab::Browse, "Browse".to_owned(), "trending-up"),
            (Tab::Saved, saved, "star"),
            (Tab::Recent, "Recent".to_owned(), "clock"),
        ] {
            let on = self.gifs.tab == tab;
            row = row.child(
                div()
                    .id(SharedString::from(format!("gif-tab|{label}")))
                    .flex_1()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .rounded(corner(9.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .text_color(if on { p.primary } else { p.muted_foreground })
                    .when(!on, |el| el.hover(|s| s.text_color(p.foreground)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.gifs.tab = tab;
                        this.gifs.scroll = ScrollHandle::new();
                        cx.notify();
                    }))
                    .child(icon(glyph).size(px(14.0)))
                    .child(label),
            );
        }
        row.into_any_element()
    }

    /// Moods to browse, and trending first.
    fn gif_browse(&mut self, key: &str, p: &Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let categories = match self.gifs.categories.get(key) {
            Some(Err(err)) => return empty("search-x", "Browsing isn't working", err, p),
            Some(Ok(list)) => Some(list.clone()),
            None => None,
        };
        let fresh = self.gifs.born.is_some_and(|t| t.elapsed() < Duration::from_millis(450));
        let mut grid = div().p(px(PAD)).flex().flex_wrap().gap(px(GAP));
        let tile_w = (INNER_W - PAD * 2.0 - GAP) / 2.0;
        let trending_picture = categories.as_ref().and_then(|c| c.first()).and_then(|c| c.preview.clone());
        grid = grid.child(arrive(
            category_tile("gif-cat|trending", "Trending", Some("trending-up"), trending_picture.as_ref(), tile_w, p)
                .on_click(cx.listener(|this, _, _, cx| this.browse_trending(cx))),
            0,
            fresh,
        ));
        match categories {
            Some(list) => {
                for (n, c) in list.into_iter().enumerate() {
                    let query = c.query.clone();
                    let tile = category_tile(
                        SharedString::from(format!("gif-cat|{}", c.query)),
                        &c.name,
                        None,
                        c.preview.as_ref(),
                        tile_w,
                        p,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.browse_category(query.clone(), window, cx)));
                    grid = grid.child(arrive(tile, n + 1, fresh));
                }
            }
            None => {
                for n in 0..9 {
                    grid = grid.child(pulse(
                        div()
                            .id(SharedString::from(format!("gif-cat-wait|{n}")))
                            .w(px(tile_w))
                            .h(px(88.0))
                            .rounded(corner(12.0))
                            .bg(p.muted),
                        SharedString::from(format!("gif-cat-pulse|{n}")),
                        60 * n as u64,
                        window,
                    ));
                }
            }
        }
        div().id("gif-browse").size_full().overflow_y_scroll().child(grid).into_any_element()
    }

    fn gif_results(&mut self, key: &str, p: &Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(r) = self.gifs.results.as_ref() else { return div().into_any_element() };
        if r.tiles.is_empty() {
            if let Some(err) = &r.failed {
                return empty("search-x", "Search isn't working", err, p);
            }
            if !r.busy && r.next.is_none() {
                return empty("search-x", &format!("No GIFs for “{}”", r.query), "Try other words.", p);
            }
        }
        let (tiles, busy, id) = (r.tiles.clone(), r.busy, format!("results|{}", r.run));
        // Near the end, the next page starts loading.
        let (offset, max) = (self.gifs.scroll.offset(), self.gifs.scroll.max_offset());
        if self.gifs.open && !busy && !tiles.is_empty() && f32::from(max.y) + f32::from(offset.y) < LOAD_AHEAD {
            self.more_gifs(cx);
        }
        self.gif_grid(&id, key, tiles, busy, false, p, window, cx)
    }

    /// Your saved GIFs, after a tile to upload one of your own.
    fn gif_saved(&mut self, key: &str, p: &Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(list) = self.gifs.saved.get(key) else { return skeletons(p, window) };
        let tiles = list.iter().filter_map(|s| s.gif.as_ref()).map(Tile::of_gif).collect();
        self.gif_grid("saved", key, tiles, false, true, p, window, cx)
    }

    /// A masonry of GIFs in two columns, each sized from its known width and
    /// height so nothing moves as they load. Only tiles near the view are drawn.
    #[allow(clippy::too_many_arguments)]
    fn gif_grid(
        &self,
        id: &str,
        key: &str,
        tiles: Vec<Tile>,
        busy: bool,
        lead: bool,
        p: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if tiles.is_empty() && !lead {
            return if busy { skeletons(p, window) } else { div().into_any_element() };
        }
        let mut ratios: Vec<f32> = Vec::with_capacity(tiles.len() + 1);
        if lead {
            ratios.push(0.6);
        }
        ratios.extend(
            tiles.iter().map(|t| if t.width > 0 && t.height > 0 { t.height as f32 / t.width as f32 } else { 1.0 }),
        );
        let (placed, height) = gifs::masonry(&ratios, INNER_W, GAP, PAD);
        let top = -f32::from(self.gifs.scroll.offset().y);
        let near = |y: f32, h: f32| y + h > top - OVERSCAN && y < top + BODY_H + OVERSCAN;
        let saved = self.gifs.saved_urls(key);
        let sending = self.gifs.sending.clone();
        let mut inner = div().relative().w_full().h(px(height));
        let mut boxes = placed.into_iter();
        if lead && let Some((x, y, w, h)) = boxes.next() {
            let up = self.gifs.uploading;
            inner = inner.child(
                div()
                    .id("gif-upload")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .rounded(corner(12.0))
                    .border_2()
                    .border_dashed()
                    .border_color(p.border)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .when(!up, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.text_color(p.primary).border_color(p.primary))
                            .on_click(cx.listener(|this, _, _, cx| this.pick_gif_file(cx)))
                    })
                    .child(if up {
                        spinning("gif-upload-spin", 20.0)
                    } else {
                        icon("upload").size(px(20.0)).into_any_element()
                    })
                    .child(if up { "Uploading…" } else { "Upload a GIF" }),
            );
        }
        for (n, (tile, (x, y, w, h))) in tiles.into_iter().zip(boxes).enumerate() {
            if !near(y, h) {
                continue;
            }
            let starred = tile.stored.as_ref().is_some_and(|u| saved.contains(u));
            let me = sending.as_deref() == Some(tile.key.as_str());
            let dimmed = sending.is_some() && !me;
            inner = inner.child(gif_tile(id, n, tile, (x, y, w, h), starred, me, dimmed, p, cx));
        }
        if busy {
            inner = inner.child(
                div()
                    .absolute()
                    .bottom(px(8.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(spinning("gif-more", 20.0)),
            );
        }
        div()
            .id(SharedString::from(format!("gif-grid|{id}")))
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.gifs.scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(inner)
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn gif_tile(
    grid: &str,
    n: usize,
    tile: Tile,
    (x, y, w, h): (f32, f32, f32, f32),
    starred: bool,
    sending: bool,
    dimmed: bool,
    p: &Palette,
    cx: &mut Context<FuwaApp>,
) -> AnyElement {
    let group = SharedString::from(format!("gif-tile|{grid}|{}", tile.key));
    let (pick, save) = (tile.clone(), tile.clone());
    // The web's `hover:ring-2 ring-primary/60`, outside the picture.
    let ring = |color: gpui_kit::Hsla| {
        vec![gpui_kit::BoxShadow {
            color,
            offset: gpui_kit::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(2.0),
            inset: false,
        }]
    };
    let lit = ring(alpha(p.primary, 0.6));
    let picture = div()
        .id(SharedString::from(format!("gif-pick|{grid}|{}", tile.key)))
        .size_full()
        .rounded(corner(12.0))
        .overflow_hidden()
        .bg(p.muted)
        .shadow(ring(alpha(p.primary, 0.0)))
        .cursor_pointer()
        // The web's `whileHover={{ scale: 1.03 }}` and `whileTap={{ scale: 0.95 }}`;
        // the one being sent sits a little smaller.
        .hover(move |s| s.shadow(lit.clone()).scale(1.03))
        .active(|s| s.scale(0.95))
        .when(sending, |el| el.scale(0.94))
        .when(dimmed, |el| el.opacity(0.45))
        .tooltip({
            let title = tile.title.clone();
            move |window, cx| {
                let say = if title.is_empty() { "Send this GIF".to_owned() } else { format!("Send {title}") };
                crate::ui::overlay::Tip::new(say).build(window, cx)
            }
        })
        .on_click(cx.listener(move |this, _, _, cx| this.pick_gif(pick.clone(), cx)))
        // An id keeps the frame it's on, which is what lets GPUI play it.
        .child(
            img(SharedString::from(tile.url.clone()))
                .id(SharedString::from(format!("gif-img|{grid}|{}", tile.key)))
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
        .when(sending, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::black().opacity(0.35))
                    .text_color(gpui_kit::white())
                    .child(spinning(SharedString::from(format!("gif-sending|{}", tile.key)), 24.0)),
            )
        });
    let star = div()
        .id(SharedString::from(format!("gif-star|{grid}|{}", tile.key)))
        .absolute()
        .top(px(6.0))
        .right(px(6.0))
        .size(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(corner(8.0))
        .bg(gpui_kit::black().opacity(0.55))
        .text_color(if starred { gpui_kit::rgb(0xfbbf24).into() } else { gpui_kit::white() })
        .cursor_pointer()
        .when(!starred, |el| el.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0)))
        .active(|s| s.scale(0.9))
        .tooltip(move |window, cx| {
            let say = if starred { "Remove from your GIFs" } else { "Save to your GIFs" };
            crate::ui::overlay::Tip::new(say).build(window, cx)
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.toggle_tile(save.clone(), cx)
        }))
        .child(icon("star").size(px(14.0)));
    // The rise makes what it moves relative, so the place is held by a box around it.
    let tile_el = div().group(group).size_full().child(picture).child(star);
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .child(grow(
            tile_el,
            SharedString::from(format!("gif-in|{grid}|{n}")),
            0.94,
            Duration::from_millis(20 * n.min(10) as u64),
        ))
        .into_any_element()
}

/// Fades in while it grows from `from` of its size, after `delay`, on the
/// web's `SPRING` (tiles rippling in one after another).
fn grow<E: gpui_kit::IntoElement + Styled + 'static>(
    el: E,
    id: SharedString,
    from: f32,
    delay: Duration,
) -> AnyElement {
    let (duration, easing) = gpui_kit::sampled_easing(gpui_kit::SpringConfig::new(520.0, 34.0, 1.0), 0.002);
    let total = delay + duration;
    let start = delay.as_secs_f32() / total.as_secs_f32().max(0.001);
    el.with_animation(
        id,
        Animation::new(total)
            .with_easing(move |t| if t <= start { 0.0 } else { easing(((t - start) / (1.0 - start)).clamp(0.0, 1.0)) }),
        move |el, t| el.opacity(t.clamp(0.0, 1.0)).scale(from + (1.0 - from) * t),
    )
    .into_any_element()
}

/// A mood's tile; they ripple in when the picker opens.
fn arrive(tile: gpui_kit::Stateful<gpui_kit::Div>, n: usize, fresh: bool) -> AnyElement {
    if !fresh {
        return tile.into_any_element();
    }
    grow(tile, SharedString::from(format!("gif-cat-in|{n}")), 0.9, Duration::from_millis(25 * n.min(12) as u64))
}

fn category_tile(
    id: impl Into<SharedString>,
    label: &str,
    glyph: Option<&'static str>,
    picture: Option<&pb::GifResult>,
    w: f32,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .relative()
        .w(px(w))
        .h(px(88.0))
        .rounded(corner(12.0))
        .overflow_hidden()
        .bg(p.muted)
        .cursor_pointer()
        // The web's `whileHover={{ scale: 1.03 }}` and `whileTap={{ scale: 0.96 }}`.
        .hover(|s| s.scale(1.03))
        .active(|s| s.scale(0.96))
        .when_some(picture, |el, picture| {
            el.child(
                img(SharedString::from(picture.preview_url.clone()))
                    .id(SharedString::from(format!("{id}|img")))
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
        .child(div().absolute().inset_0().bg(gpui_kit::linear_gradient(
            0.0,
            gpui_kit::linear_color_stop(gpui_kit::black().opacity(0.7), 0.0),
            gpui_kit::linear_color_stop(gpui_kit::black().opacity(0.1), 1.0),
        )))
        .child(
            div()
                .absolute()
                .bottom(px(8.0))
                .left(px(10.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_sm()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(gpui_kit::white())
                .when_some(glyph, |el, glyph| el.child(icon(glyph).size(px(16.0))))
                .child(label.to_owned()),
        )
}

fn empty(glyph: &str, title: &str, text: &str, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .size_full()
            .px(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .child(
                div()
                    .size(px(48.0))
                    .mb(px(4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(16.0))
                    .bg(p.muted)
                    .text_color(p.muted_foreground)
                    .child(icon(glyph).size(px(24.0))),
            )
            .child(div().text_sm().font_weight(FontWeight::BOLD).child(title.to_owned()))
            .child(div().text_xs().text_center().text_color(p.muted_foreground).child(text.to_owned())),
        SharedString::from(format!("gif-empty|{title}")),
        Duration::ZERO,
        8.0,
    )
    .into_any_element()
}

fn skeletons(p: &Palette, window: &Window) -> AnyElement {
    let (placed, _) = gifs::masonry(&[0.9, 1.25, 1.15, 0.85, 1.35, 1.0], INNER_W, GAP, PAD);
    let mut inner = div().relative().size_full();
    for (n, (x, y, w, h)) in placed.into_iter().enumerate() {
        inner = inner.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).child(pulse(
            div().size_full().rounded(corner(12.0)).bg(p.muted),
            SharedString::from(format!("gif-wait|{n}")),
            70 * n as u64,
            window,
        )));
    }
    inner.into_any_element()
}

/// Tailwind's `animate-pulse` (half see-through at the middle of every two
/// seconds), `delay` milliseconds behind, for tiles still coming.
fn pulse<E: gpui_kit::IntoElement + Styled + 'static>(
    el: E,
    id: SharedString,
    delay: u64,
    window: &Window,
) -> AnyElement {
    const PERIOD: f32 = 2000.0;
    motion::ambient(el, id, Duration::from_millis(PERIOD as u64), window, move |el, t| {
        // `cubic-bezier(0.4, 0, 0.6, 1)`, near enough as a cosine.
        let t = (t - delay as f32 / PERIOD).rem_euclid(1.0);
        el.opacity(1.0 - 0.25 * (1.0 - (t * std::f32::consts::TAU).cos()))
    })
}

fn spinning(id: impl Into<SharedString>, size: f32) -> AnyElement {
    icon("loader-circle")
        .size(px(size))
        .with_animation(
            gpui_kit::ElementId::Name(id.into()),
            Animation::new(Duration::from_millis(900)).repeat(),
            |el, t| el.rotate(gpui_kit::percentage(t)),
        )
        .into_any_element()
}

// ───────────────────────── In messages ─────────────────────────

/// A GIF in a message: sized before it loads, so the list doesn't jump, with
/// a star to save it and its provider's credit on hover.
pub(crate) fn gif_in_message(
    mid: &str,
    gif: &pb::MessageGif,
    starred: bool,
    p: &Palette,
    this: &WeakEntity<FuwaApp>,
    key: &str,
) -> AnyElement {
    let (w, h) = gifs::fit(gif.width, gif.height);
    let group = SharedString::from(format!("gif-msg|{mid}"));
    let credit = gifs::provider_name(gif.provider);
    let (this, key, url) = (this.clone(), key.to_owned(), gif.url.clone());
    let star = div()
        .id(SharedString::from(format!("gif-msg-star|{mid}")))
        .absolute()
        .top(px(6.0))
        .right(px(6.0))
        .size(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(corner(9.0))
        .bg(gpui_kit::black().opacity(0.55))
        .text_color(if starred { gpui_kit::rgb(0xfbbf24).into() } else { gpui_kit::white() })
        .cursor_pointer()
        .when(!starred, |el| el.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0)))
        .active(|s| s.scale(0.9))
        .tooltip(move |window, cx| {
            let say = if starred { "Remove from your GIFs" } else { "Save to your GIFs" };
            crate::ui::overlay::Tip::new(say).build(window, cx)
        })
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let _ = this.update(cx, |this, cx| this.toggle_saved_gif(key.clone(), url.clone(), cx));
        })
        .child(icon("star").size(px(16.0)));
    div()
        .id(SharedString::from(format!("gif-msg-box|{mid}")))
        .group(group.clone())
        .relative()
        .mt(px(4.0))
        .w(px(w))
        .h(px(h))
        .rounded(corner(12.0))
        .overflow_hidden()
        .bg(p.muted)
        // `shadow-sm`.
        .shadow(vec![gpui_kit::BoxShadow {
            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.05),
            offset: gpui_kit::point(px(0.0), px(1.0)),
            blur_radius: px(2.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        .child(
            img(SharedString::from(gif.url.clone()))
                .id(SharedString::from(format!("gif-msg-img|{mid}")))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback({
                    let fg = p.muted_foreground;
                    move || {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(fg)
                            .child("GIF")
                            .into_any_element()
                    }
                }),
        )
        .child(star)
        .when(!credit.is_empty(), |el| {
            el.child(
                div()
                    .id(SharedString::from(format!("gif-msg-credit|{mid}")))
                    .absolute()
                    .right(px(8.0))
                    .bottom(px(6.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui_kit::white().opacity(0.85))
                    .opacity(0.0)
                    .group_hover(group, |s| s.opacity(1.0))
                    .child(format!("via {credit}")),
            )
        })
        .into_any_element()
}

/// The open place's saved GIF links, for the stars on its messages.
pub(crate) fn saved_here(app: &FuwaApp, key: &str) -> Rc<HashSet<String>> {
    Rc::new(app.gifs.saved_urls(key))
}
