//! The far-left rail: home, then each instance followed by its servers in
//! the order you arranged them, folders and all, then adding something. A
//! pill on the left edge grows with what's under it, as on Discord: tall for
//! the open server, half for the hovered one, a dot for unread ones. Like
//! the web app's `Rail.tsx` and `RailFolder.tsx`, over `core/rail.rs`.
//!
//! Servers and folders drag into place (`hooks/use-rail-arrange.ts`): a
//! server goes between any two, into a folder (onto a closed one, or between
//! an open one's servers), or onto another server to make a folder of the
//! two; a folder goes between the top-level places. The places have fixed
//! heights, so where each sits is worked out from the layout ([`slots`]) and
//! the others slide aside on springs while one is held.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, DragMoveEvent, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, Rgba, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::rail::{
    self, FOLDER_COLORS, FolderChange, RailDrop, RailEntry, RailFolder, RailLayout, folder_label, key_of, rail_layout,
};
use crate::core::store::Connection;
use crate::pb;
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::context_menu::{Built, Item, MenuOf, run};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::widgets::{badge, conn_dot, fuwa_mark, icon, initials, pal, server_icon};

pub const RAIL: f32 = 72.0;
/// A server's (or a folder's) place: 48px, then the rail's 8px gap.
const ICON: f32 = 48.0;
const GAP: f32 = 8.0;

/// The web's `.server-icon`: a circle that settles into a rounded square
/// (32% of its size) when open or pointed at.
fn icon_radius(size: f32, square: bool) -> f32 {
    if square { size * 0.32 } else { size / 2.0 }
}

/// A folder's color, or the theme's accent.
fn folder_color(color: u32, p: &Palette) -> Rgba {
    if color == 0 { p.primary } else { gpui_kit::rgb(color) }
}

struct RailInstance {
    key: String,
    name: String,
    connection: Connection,
    signed_in: bool,
    servers: HashMap<String, (pb::Server, u32)>,
    layout: RailLayout,
    /// Unread direct messages there, counted on its chip.
    dm_unread: u32,
}

/// Something on the rail as drawn, in its instance's group's own coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct RailSlot {
    pub kind: SlotKind,
    pub id: String,
    /// The folder a server is in, or "".
    pub folder: String,
    pub open: bool,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    Server,
    /// A folder's icon.
    Folder,
    /// A top-level place: a server, or a folder with its open servers.
    Unit,
}

/// Where every server, folder icon and top-level place sits, top to bottom.
pub fn slots(layout: &[RailEntry], open: impl Fn(&str) -> bool) -> Vec<RailSlot> {
    let mut out = Vec::new();
    let mut y = 0.0;
    let slot = |kind, id: &str, folder: &str, is_open, top: f32, bottom: f32| RailSlot {
        kind,
        id: id.to_owned(),
        folder: folder.to_owned(),
        open: is_open,
        top,
        bottom,
    };
    for e in layout {
        let start = y;
        match e {
            RailEntry::Server(id) => {
                out.push(slot(SlotKind::Server, id, "", false, y, y + ICON));
                y += ICON + GAP;
            }
            RailEntry::Folder(f) => {
                let is_open = open(&f.id);
                out.push(slot(SlotKind::Folder, &f.id, "", is_open, y, y + ICON));
                y += ICON + GAP;
                if is_open {
                    for s in &f.servers {
                        out.push(slot(SlotKind::Server, s, &f.id, false, y, y + ICON));
                        y += ICON + GAP;
                    }
                }
            }
        }
        out.push(slot(SlotKind::Unit, key_of(e), "", false, start, y - GAP));
    }
    out
}

/// Where the held one would land: `at` is its place among the others (they
/// slide to open it); `ring` names one to drop onto.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    key: String,
    pub drop: Option<RailDrop>,
    pub at: usize,
    pub ring: Option<String>,
}

/// One drag, worked out from where everything was when it was picked up.
#[derive(Debug, Clone)]
pub struct Held {
    folder: bool,
    id: String,
    /// The others, in order.
    rest: Vec<RailSlot>,
    /// How many of the others were above it.
    from: usize,
    /// How far the others slide: its height and the gap after it.
    size: f32,
}

impl Held {
    pub fn new(all: &[RailSlot], id: &str, folder: bool) -> Option<Self> {
        let pool: Vec<&RailSlot> =
            all.iter().filter(|s| if folder { s.kind == SlotKind::Unit } else { s.kind != SlotKind::Unit }).collect();
        let me: &RailSlot = pool.iter().find(|s| s.id == id && (folder || s.kind == SlotKind::Server))?;
        let rest: Vec<RailSlot> = pool.iter().filter(|s| !std::ptr::eq(**s, me)).map(|s| (*s).clone()).collect();
        let from = rest.iter().filter(|s| s.top < me.top).count();
        Some(Self { folder, id: id.to_owned(), rest, from, size: me.bottom - me.top + GAP })
    }

    /// How far the n-th of the others sits from its place while the held one would land at `place`.
    pub fn shift(&self, n: usize, place: usize) -> f32 {
        (if n >= self.from { -self.size } else { 0.0 }) + if n >= place { self.size } else { 0.0 }
    }

    /// Where the pointer (at `py` in the group) would put the held one,
    /// going by where the others show now (`shown`).
    pub fn mark(&self, layout: &[RailEntry], shown: Option<&Mark>, py: f32) -> Mark {
        let id = self.id.clone();
        let place = shown.map_or(self.from, |m| m.at);
        let rest = &self.rest;
        if rest.is_empty() {
            return Mark { key: "none".into(), drop: None, at: 0, ring: None };
        }
        let tops: Vec<f32> = rest.iter().enumerate().map(|(n, s)| s.top + self.shift(n, place)).collect();
        let next_top_level = |after: &str| -> Option<String> {
            let keys: Vec<&str> = layout.iter().map(key_of).filter(|k| *k != id).collect();
            let n = keys.iter().position(|k| *k == after)?;
            keys.get(n + 1).map(|k| (*k).to_owned())
        };
        let folder_servers = |folder: &str| -> Vec<String> {
            layout
                .iter()
                .find_map(|e| match e {
                    RailEntry::Folder(f) if f.id == folder => Some(f.servers.clone()),
                    _ => None,
                })
                .unwrap_or_default()
                .into_iter()
                .filter(|s| *s != id)
                .collect()
        };
        let before = |n: usize| -> Mark {
            let s = &rest[n];
            if s.kind == SlotKind::Server && !s.folder.is_empty() {
                return Mark {
                    key: format!("in:{}:{}", s.folder, s.id),
                    drop: Some(RailDrop::Server {
                        id: id.clone(),
                        folder: s.folder.clone(),
                        before: Some(s.id.clone()),
                    }),
                    at: n,
                    ring: None,
                };
            }
            Mark {
                key: format!("before:{}", s.id),
                drop: Some(RailDrop::Server { id: id.clone(), folder: String::new(), before: Some(s.id.clone()) }),
                at: n,
                ring: None,
            }
        };
        let folder_at = |n: usize| -> Mark {
            let s = rest.get(n);
            Mark {
                key: format!("folder:{}", s.map_or("end", |s| s.id.as_str())),
                drop: Some(RailDrop::Folder { id: id.clone(), before: s.map(|s| s.id.clone()) }),
                at: n,
                ring: None,
            }
        };
        let hit = rest.iter().enumerate().position(|(n, s)| py >= tops[n] && py < tops[n] + (s.bottom - s.top));
        let Some(hit) = hit else {
            if py < tops[0] {
                return if self.folder { folder_at(0) } else { before(0) };
            }
            let last = rest.len() - 1;
            let end = tops[last] + (rest[last].bottom - rest[last].top);
            if py > end + self.size / 2.0 {
                return if self.folder {
                    folder_at(rest.len())
                } else {
                    Mark {
                        key: "end".into(),
                        drop: Some(RailDrop::Server { id: id.clone(), folder: String::new(), before: None }),
                        at: rest.len(),
                        ring: None,
                    }
                };
            }
            // In a gap: the place that's open stays.
            return shown.cloned().unwrap_or(Mark { key: String::new(), drop: None, at: place, ring: None });
        };
        let s = &rest[hit];
        let f = (py - tops[hit]) / (s.bottom - s.top);
        if self.folder {
            return if f < 0.5 { folder_at(hit) } else { folder_at(hit + 1) };
        }
        if s.kind == SlotKind::Server && s.folder.is_empty() {
            if f < 0.25 {
                return before(hit);
            }
            if f > 0.75 {
                return Mark {
                    key: format!("after:{}", s.id),
                    drop: Some(RailDrop::Server {
                        id: id.clone(),
                        folder: String::new(),
                        before: next_top_level(&s.id),
                    }),
                    at: hit + 1,
                    ring: None,
                };
            }
            return Mark {
                key: format!("with:{}", s.id),
                drop: Some(RailDrop::Combine { id: id.clone(), with: s.id.clone() }),
                at: place,
                ring: Some(s.id.clone()),
            };
        }
        if s.kind == SlotKind::Server {
            if f < 0.5 {
                return before(hit);
            }
            let siblings = folder_servers(&s.folder);
            let after = siblings.iter().position(|x| *x == s.id).and_then(|n| siblings.get(n + 1)).cloned();
            return Mark {
                key: format!("in:{}:{}", s.folder, after.as_deref().unwrap_or("")),
                drop: Some(RailDrop::Server { id: id.clone(), folder: s.folder.clone(), before: after }),
                at: hit + 1,
                ring: None,
            };
        }
        // A folder's icon: its top edge is the place above it; the rest goes in.
        if f < if s.open { 0.3 } else { 0.25 } {
            return before(hit);
        }
        if !s.open {
            return Mark {
                key: format!("into:{}", s.id),
                drop: Some(RailDrop::Server { id: id.clone(), folder: s.id.clone(), before: None }),
                at: place,
                ring: Some(s.id.clone()),
            };
        }
        let first = folder_servers(&s.id).into_iter().next();
        Mark {
            key: format!("in:{}:{}", s.id, first.as_deref().unwrap_or("")),
            drop: Some(RailDrop::Server { id: id.clone(), folder: s.id.clone(), before: first }),
            at: hit + 1,
            ring: None,
        }
    }
}

/// The rail's state between frames: the drag going on, and where the add button is.
#[derive(Default)]
pub(crate) struct RailState {
    /// The instance being arranged, what's held and where it would land.
    held: Option<(String, Held, Option<Mark>)>,
    /// Each instance's places as last drawn.
    slots: HashMap<String, Vec<RailSlot>>,
    /// Bumped by each drop, so the slides start over from where things now are.
    settled: u64,
    /// The add button's box, for its menu.
    add_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The folder being renamed and recolored, with its name field and the color picked.
    pub(crate) editing: Option<FolderEdit>,
    /// When something was last dragged here: the click that ends a drag mustn't open what was dropped.
    moved_at: Option<std::time::Instant>,
}

pub(crate) struct FolderEdit {
    key: String,
    folder: String,
    name: gpui_kit::Entity<gpui_kit::component::input::InputState>,
    color: u32,
    _sub: gpui_kit::Subscription,
}

/// What's held: a server's icon or a folder's tile, following the pointer.
#[derive(Clone)]
pub struct RailDrag {
    pub key: String,
    pub id: String,
    pub folder: bool,
    server: Option<pb::Server>,
    /// A folder's first servers and its color.
    tiles: Vec<pb::Server>,
    color: u32,
}

impl Render for RailDrag {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        // Over a server or folder it would go into, it shrinks toward it.
        let over = cx.try_global::<RailOver>().is_some_and(|o| o.0);
        let scale = motion::follow(
            SharedString::from(format!("rail-drag|{}", self.id)),
            if over { 0.78 } else { 1.1 },
            window,
            cx,
        );
        let size = ICON * scale;
        let radius = size * 0.32;
        let face = match &self.server {
            Some(server) => server_icon(server, size, radius, &p).into_any_element(),
            None => folder_tile(&self.tiles, false, folder_color(self.color, &p), size / ICON, &p).into_any_element(),
        };
        div().w(px(RAIL)).h(px(ICON)).flex().items_center().justify_center().child(
            div()
                .rounded(px(radius))
                .shadow(vec![
                    gpui_kit::BoxShadow {
                        color: alpha(p.primary, 0.55),
                        offset: gpui_kit::point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(2.0),
                        inset: false,
                    },
                    gpui_kit::BoxShadow {
                        color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.6),
                        offset: gpui_kit::point(px(0.0), px(18.0)),
                        blur_radius: px(30.0),
                        spread_radius: px(-12.0),
                        inset: false,
                    },
                ])
                .child(face),
        )
    }
}

/// Whether the held one is over something it would go into, for its look.
#[derive(Default)]
struct RailOver(bool);

impl gpui_kit::Global for RailOver {}

/// A folder's face: its first servers in a 2×2 tile while closed, a folder
/// icon in its color while open (the web's `.rail-folder`).
fn folder_tile(servers: &[pb::Server], open: bool, color: Rgba, scale: f32, p: &Palette) -> gpui_kit::Div {
    let size = ICON * scale;
    let mini = 16.0 * scale;
    let mut grid = div().w(px(mini * 2.0 + 3.2 * scale)).flex().flex_wrap().gap(px(3.2 * scale));
    for s in servers.iter().take(4) {
        grid = grid.child(server_icon(s, mini, mini * 0.35, p));
    }
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(16.0 * scale))
        .when(!open, |el| el.bg(mix(p.background, color, 0.3)).border_1().border_color(alpha(color, 0.35)))
        .when(!open, |el| el.child(grid))
        .when(open, |el| el.child(icon("folder-open").size(px(24.0 * scale)).text_color(color)))
}

impl FuwaApp {
    pub(crate) fn render_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let prefs_open = self.prefs.rail_open.clone();
        let instances: Vec<RailInstance> = self.core.shared.read(|s| {
            s.order
                .iter()
                .filter_map(|k| s.instance(k))
                .map(|i| {
                    let ids: Vec<String> = i.servers.iter().map(|sv| sv.id.clone()).collect();
                    RailInstance {
                        key: i.key.clone(),
                        name: i.name(),
                        connection: i.connection,
                        signed_in: i.me.is_some(),
                        servers: i
                            .servers
                            .iter()
                            .map(|sv| (sv.id.clone(), (sv.clone(), i.server_unread(&sv.id))))
                            .collect(),
                        layout: rail_layout(i.rail.as_deref(), &ids),
                        dm_unread: i.dms.unread.values().sum(),
                    }
                })
                .collect()
        });
        if !cx.has_active_drag() {
            self.rail.held = None;
            cx.set_global(RailOver(false));
        }

        // The cloud is lit only on the page with no instance (the web's welcome).
        let home_active = matches!(self.nav, Nav::Home { dm: None });
        let mut list = div().flex().flex_col().items_center().gap(px(GAP)).pt(px(12.0)).pb(px(12.0));

        // Home: the little cloud.
        let home_hovered = self.hovered.as_deref() == Some("home");
        let home_radius = motion::follow("home|r", icon_radius(48.0, home_active || home_hovered), window, cx);
        list = list.child(
            self.rail_item(
                "home".into(),
                home_active,
                false,
                0,
                Nav::Home { dm: None },
                div()
                    .size(px(48.0))
                    .rounded(px(home_radius))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.card)
                    .child(fuwa_mark(32.0, &p)),
                "Direct messages",
                48.0,
                window,
                cx,
            ),
        );

        for (n, inst) in instances.iter().enumerate() {
            list = list.child(divider(&p));
            list = list.child(self.instance_chip(inst, n, window, cx));
            let open = |f: &str| prefs_open.contains(&format!("{}/{f}", inst.key));
            list = list.child(self.arranged_servers(inst, n, &open, window, cx));
            // Servers you applied to, waiting to be let in (`ui::join`).
            for applied in self.applied_buttons(&inst.key, window, cx) {
                list = list.child(applied);
            }
        }

        // Adding a server or an instance, under a divider.
        let add_hovered = self.hovered.as_deref() == Some("rail-add");
        let lit = add_hovered;
        let add_radius = motion::follow("rail-add|r", icon_radius(48.0, lit), window, cx);
        let add_turn = motion::follow("rail-add|turn", if add_hovered { 90.0 } else { 0.0 }, window, cx);
        let add_at = self.rail.add_at.clone();
        let add = div()
            .id("rail-add")
            .relative()
            .size(px(48.0))
            .rounded(px(add_radius))
            .flex()
            .items_center()
            .justify_center()
            .bg(if lit { p.primary } else { p.card })
            .text_color(if lit { p.primary_foreground } else { p.primary })
            .cursor_pointer()
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                if *on {
                    this.hovered = Some("rail-add".into());
                } else if this.hovered.as_deref() == Some("rail-add") {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .active(|s| s.opacity(0.92))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.context.as_ref().is_some_and(|m| matches!(m.of, MenuOf::RailAdd)) {
                    this.close_context_menu(cx);
                    return;
                }
                // The web's side="right" align="start": beside the button, level with its top.
                let at = this
                    .rail
                    .add_at
                    .get()
                    .map(|b| gpui_kit::point(b.right() + px(5.0), b.top()))
                    .unwrap_or_else(|| window.mouse_position());
                this.open_context_menu(MenuOf::RailAdd, at, window, cx);
            }))
            .child(gpui_kit::canvas(move |bounds, _, _| add_at.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0())
            .child(
                gpui_kit::svg()
                    .path("icons/plus.svg")
                    .size(px(24.0))
                    .text_color(if lit { p.primary_foreground } else { p.primary })
                    .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(add_turn.to_radians()))),
            );

        div()
            .w(px(RAIL))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(p.rail_surface)
            .child(div().id("rail-scroll").flex_1().overflow_y_scroll().child(list.child(divider(&p)).child(add)))
    }

    /// An instance's servers in the order you arranged them, folders and all.
    fn arranged_servers(
        &mut self,
        inst: &RailInstance,
        n: usize,
        open: &dyn Fn(&str) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let all = slots(&inst.layout, open);
        self.rail.slots.insert(inst.key.clone(), all.clone());
        // While one of these is held: how far each of the others slides.
        let held = self.rail.held.as_ref().filter(|(k, _, _)| *k == inst.key).map(|(_, h, m)| (h.clone(), m.clone()));
        let settled = self.rail.settled;
        let shift_of = |kind: SlotKind, id: &str, folder: &str| -> f32 {
            let Some((held, mark)) = &held else { return 0.0 };
            let place = mark.as_ref().map_or(held.from, |m| m.at);
            held.rest
                .iter()
                .position(|s| {
                    s.id == id
                        && if held.folder { s.kind == SlotKind::Unit } else { s.kind == kind && s.folder == folder }
                })
                .map_or(0.0, |at| held.shift(at, place))
        };
        let ring = held.as_ref().and_then(|(_, m)| m.as_ref()).and_then(|m| m.ring.clone());
        let held_id = held.as_ref().map(|(h, _)| h.id.clone());
        let active = match &self.nav {
            Nav::Server { key, server } if *key == inst.key => Some(server.clone()),
            _ => None,
        };
        let can_arrange = inst.signed_in && inst.connection == Connection::Live;

        let mut group =
            div().id(SharedString::from(format!("rail-group|{}", inst.key))).w_full().flex().flex_col().gap(px(GAP));
        let mut m = 0usize;
        for e in &inst.layout {
            match e {
                RailEntry::Server(id) => {
                    let Some((server, unread)) = inst.servers.get(id) else { continue };
                    let slide = if held.as_ref().is_some_and(|(h, _)| h.folder) {
                        shift_of(SlotKind::Unit, id, "")
                    } else {
                        shift_of(SlotKind::Server, id, "")
                    };
                    let item = self.server_button(
                        &inst.key,
                        server,
                        *unread,
                        active.as_deref() == Some(id.as_str()),
                        "",
                        can_arrange,
                        held_id.as_deref() == Some(id.as_str()),
                        ring.as_deref() == Some(id.as_str()),
                        slide,
                        settled,
                        window,
                        cx,
                    );
                    group = group.child(motion::rise(
                        div().child(item),
                        SharedString::from(format!("s|{}|{}|in", inst.key, id)),
                        Duration::from_millis(40 * (n + m) as u64),
                        10.0,
                    ));
                }
                RailEntry::Folder(f) => {
                    let unit = self.folder_block(
                        inst,
                        f,
                        open(&f.id),
                        active.as_deref(),
                        can_arrange,
                        &held,
                        ring.as_deref(),
                        &shift_of,
                        settled,
                        window,
                        cx,
                    );
                    group = group.child(motion::rise(
                        div().child(unit),
                        SharedString::from(format!("f|{}|{}|in", inst.key, f.id)),
                        Duration::from_millis(40 * (n + m) as u64),
                        10.0,
                    ));
                }
            }
            m += 1;
        }
        let key = inst.key.clone();
        let key2 = inst.key.clone();
        group
            .on_drag_move::<RailDrag>(
                cx.listener(move |this, event: &DragMoveEvent<RailDrag>, _, cx| this.rail_drag_moved(&key, event, cx)),
            )
            .on_drop::<RailDrag>(cx.listener(move |this, drag: &RailDrag, _, cx| this.rail_dropped(&key2, drag, cx)))
            .into_any_element()
    }

    /// A folder: its tile (or, open, its icon over its servers on a backdrop of its color).
    #[allow(clippy::too_many_arguments)]
    fn folder_block(
        &mut self,
        inst: &RailInstance,
        f: &RailFolder,
        open: bool,
        active: Option<&str>,
        can_arrange: bool,
        held: &Option<(Held, Option<Mark>)>,
        ring: Option<&str>,
        shift_of: &dyn Fn(SlotKind, &str, &str) -> f32,
        settled: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let color = folder_color(f.color, &p);
        let id = format!("f|{}|{}", inst.key, f.id);
        let name_of = |s: &str| inst.servers.get(s).map(|(sv, _)| sv.name.clone());
        let label = folder_label(f, name_of);
        let unread: u32 = f
            .servers
            .iter()
            .filter(|s| active != Some(s.as_str()))
            .filter_map(|s| inst.servers.get(s).map(|(_, u)| *u))
            .sum();
        let active_inside = active.is_some_and(|a| f.servers.iter().any(|s| s == a));
        let of = MenuOf::RailFolder { key: inst.key.clone(), folder: f.id.clone() };
        let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit());
        let hovered = (self.hovered.as_deref() == Some(id.as_str()) || lit) && !cx.has_active_drag();
        let folder_drag = held.as_ref().is_some_and(|(h, _)| h.folder);
        let lifted = held.as_ref().is_some_and(|(h, _)| h.folder && h.id == f.id);
        let unit_slide = if folder_drag { shift_of(SlotKind::Unit, &f.id, "") } else { 0.0 };
        let icon_slide = if folder_drag { 0.0 } else { shift_of(SlotKind::Folder, &f.id, "") };
        let pill = motion::follow_bouncy(
            SharedString::from(format!("{id}|pill")),
            if !open && active_inside {
                40.0
            } else if hovered {
                20.0
            } else if !open && unread > 0 {
                8.0
            } else {
                0.0
            },
            window,
            cx,
        );
        let target = ring == Some(f.id.as_str());
        let grow =
            motion::follow(SharedString::from(format!("{id}|target")), if target { 1.12 } else { 1.0 }, window, cx);
        let tiles: Vec<pb::Server> =
            f.servers.iter().filter_map(|s| inst.servers.get(s).map(|(sv, _)| sv.clone())).collect();
        let face = folder_tile(&tiles, open, color, grow, &p).when(target, |el| {
            el.shadow(vec![
                gpui_kit::BoxShadow {
                    color: p.background.into(),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(3.0),
                    inset: false,
                },
                gpui_kit::BoxShadow {
                    color: color.into(),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(5.0),
                    inset: false,
                },
            ])
        });
        let drag = RailDrag {
            key: inst.key.clone(),
            id: f.id.clone(),
            folder: true,
            server: None,
            tiles: tiles.clone(),
            color: f.color,
        };
        let hover_id = id.clone();
        let (key, fid) = (inst.key.clone(), f.id.clone());
        let icon_row = div()
            .id(SharedString::from(format!("{id}|icon")))
            .relative()
            .w(px(RAIL))
            .h(px(ICON))
            .flex()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                if *on {
                    this.hovered = Some(hover_id.clone());
                } else if this.hovered.as_deref() == Some(hover_id.as_str()) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.rail.moved_at.is_some_and(|at| at.elapsed() < Duration::from_millis(300)) {
                    return;
                }
                let open = this.core.folder_open(&key, &fid);
                this.core.set_folder_open(&key, &fid, !open);
                this.prefs = this.core.prefs();
                cx.notify();
            }))
            .when(inst.signed_in, |el| el.on_mouse_down(gpui_kit::MouseButton::Right, self.right_click(of.clone(), cx)))
            .when(can_arrange, |el| el.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(ICON / 2.0 - pill / 2.0))
                    .w(px(4.0))
                    .h(px(pill.max(0.0)))
                    .rounded_r(px(4.0))
                    .bg(p.foreground)
                    .opacity((pill / 8.0).clamp(0.0, 1.0)),
            )
            .child(div().relative().child(face))
            .when(unread > 0 && !open, |el| {
                el.child(div().absolute().right(px(8.0)).bottom(px(-4.0)).child(badge(unread, &p)))
            })
            .when(hovered && self.context.is_none(), |el| el.child(tooltip(&id, &label, ICON, &p)));
        let icon_row = slid(icon_row, &format!("{id}|icon"), icon_slide, settled, window, cx);

        let mut unit = div().relative().w_full().flex().flex_col().gap(px(GAP));
        // The open folder's backdrop, stretching to its servers.
        let shown = motion::follow(SharedString::from(format!("{id}|bg")), if open { 1.0 } else { 0.0 }, window, cx);
        if shown > 0.01 {
            let into = held.as_ref().and_then(|(_, m)| m.as_ref()).is_some_and(|m| {
                m.ring.is_none() && matches!(&m.drop, Some(RailDrop::Server { folder, .. }) if *folder == f.id)
            });
            let from_here = held.as_ref().is_some_and(|(h, _)| !h.folder && f.servers.contains(&h.id));
            let extra = if into { ICON + GAP } else { 0.0 } - if from_here { ICON + GAP } else { 0.0 };
            let extra = motion::follow(SharedString::from(format!("{id}|bg-grow|{settled}")), extra, window, cx);
            unit = unit.child(
                div()
                    .absolute()
                    .top(px(-4.0 + icon_slide))
                    .left(px(RAIL / 2.0 - 28.0))
                    .w(px(56.0))
                    .h(px((1.0 + f.servers.len() as f32) * (ICON + GAP) - GAP + 8.0 + extra))
                    .rounded(px(18.0))
                    .bg(alpha(color, 0.18 * shown)),
            );
        }
        unit = unit.child(icon_row);
        if open {
            for (k, s) in f.servers.iter().enumerate() {
                let Some((server, unread)) = inst.servers.get(s) else { continue };
                let item = self.server_button(
                    &inst.key,
                    server,
                    *unread,
                    active == Some(s.as_str()),
                    &f.id,
                    can_arrange,
                    !folder_drag && held.as_ref().is_some_and(|(h, _)| h.id == *s),
                    false,
                    if folder_drag { 0.0 } else { shift_of(SlotKind::Server, s, &f.id) },
                    settled,
                    window,
                    cx,
                );
                unit = unit.child(motion::rise(
                    div().child(item),
                    SharedString::from(format!("{id}|{s}|in")),
                    Duration::from_millis(30 * k as u64),
                    -8.0,
                ));
            }
        }
        let unit = unit.when(lifted, |el| el.opacity(0.0));
        slid(unit, &format!("{id}|unit"), unit_slide, settled, window, cx).into_any_element()
    }

    /// One server on the rail, with its pill, name and unread count; it drags into place.
    #[allow(clippy::too_many_arguments)]
    fn server_button(
        &mut self,
        key: &str,
        server: &pb::Server,
        unread: u32,
        active: bool,
        folder: &str,
        can_arrange: bool,
        lifted: bool,
        target: bool,
        slide: f32,
        settled: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let id = format!("s|{}|{}", key, server.id);
        let hovered = self.hovered.as_deref() == Some(id.as_str()) && !cx.has_active_drag();
        let radius = motion::follow(
            SharedString::from(format!("{id}|r")),
            icon_radius(48.0, active || hovered || target),
            window,
            cx,
        );
        let grow =
            motion::follow(SharedString::from(format!("{id}|target")), if target { 1.12 } else { 1.0 }, window, cx);
        let face = server_icon(server, 48.0 * grow, radius * grow, &p).when(target, |el| {
            el.shadow(vec![
                gpui_kit::BoxShadow {
                    color: p.background.into(),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(3.0),
                    inset: false,
                },
                gpui_kit::BoxShadow {
                    color: p.primary.into(),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(5.0),
                    inset: false,
                },
            ])
        });
        let item = self.rail_item(
            id.clone(),
            active,
            unread > 0,
            if active { 0 } else { unread },
            Nav::Server { key: key.to_owned(), server: server.id.clone() },
            face,
            &server.name,
            48.0,
            window,
            cx,
        );
        let drag = RailDrag {
            key: key.to_owned(),
            id: server.id.clone(),
            folder: false,
            server: Some(server.clone()),
            tiles: Vec::new(),
            color: 0,
        };
        let el = div()
            .id(SharedString::from(format!("{id}|{folder}|drag")))
            .h(px(ICON))
            .flex()
            .items_center()
            .justify_center()
            .when(can_arrange, |el| el.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone())))
            .when(lifted, |el| el.opacity(0.0))
            .child(item);
        slid(el, &format!("{id}|{folder}"), slide, settled, window, cx).into_any_element()
    }

    /// One thing on the rail, with its pill, its name beside it and its badge.
    #[allow(clippy::too_many_arguments)]
    fn rail_item(
        &mut self,
        id: String,
        active: bool,
        unread: bool,
        count: u32,
        nav: Nav,
        face: gpui_kit::Div,
        name: &str,
        size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        let of = match &nav {
            Nav::Server { key, server } => Some(MenuOf::Server { key: key.clone(), server: server.clone() }),
            _ => None,
        };
        // A server whose menu is open stays lit as if hovered.
        let lit = of.as_ref().is_some_and(|of| self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit()));
        let hovered = (self.hovered.as_deref() == Some(id.as_str()) || lit) && !cx.has_active_drag();
        let pill = motion::follow_bouncy(
            SharedString::from(format!("{id}|pill")),
            if active {
                40.0
            } else if hovered {
                20.0
            } else if unread {
                8.0
            } else {
                0.0
            },
            window,
            cx,
        );
        let hover_id = id.clone();
        let hover_of = of.clone();
        div()
            .id(SharedString::from(id.clone()))
            .relative()
            .w(px(RAIL))
            .h(px(size))
            .flex()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(hover_id.clone());
                } else if this.hovered.as_deref() == Some(hover_id.as_str()) {
                    this.hovered = None;
                }
                if let Some(of) = &hover_of {
                    this.set_hover_target(of.clone(), *hovered);
                }
                cx.notify();
            }))
            .when_some(of, |el, of| el.on_mouse_down(gpui_kit::MouseButton::Right, self.right_click(of, cx)))
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.rail.moved_at.is_some_and(|at| at.elapsed() < Duration::from_millis(300)) {
                    return;
                }
                this.navigate(nav.clone(), window, cx)
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(size / 2.0 - pill / 2.0))
                    .w(px(4.0))
                    .h(px(pill.max(0.0)))
                    .rounded_r(px(4.0))
                    .bg(p.foreground)
                    .opacity((pill / 8.0).clamp(0.0, 1.0)),
            )
            .child(div().relative().flex().items_center().justify_center().child(face.overflow_hidden()))
            .when(count > 0, |el| el.child(div().absolute().right(px(8.0)).bottom(px(-4.0)).child(badge(count, &p))))
            .when(hovered && self.context.is_none(), |el| el.child(tooltip(&id, name, size, &p)))
    }

    /// An instance's own little chip: its initials and how its connection is doing.
    fn instance_chip(
        &mut self,
        inst: &RailInstance,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        // Its page, Friends and its conversations are all "here, but not in a server", as on the web.
        let active = match &self.nav {
            Nav::Instance { key } | Nav::Friends { key } | Nav::Home { dm: Some((key, _)) } => *key == inst.key,
            _ => false,
        };
        let id = format!("i|{}", inst.key);
        let hovered = self.hovered.as_deref() == Some(id.as_str());
        let radius =
            motion::follow(SharedString::from(format!("{id}|r")), icon_radius(36.0, active || hovered), window, cx);
        // The web's small chip: 36px on the card, the instance's initials in the muted color.
        let face = div()
            .relative()
            .size(px(36.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            .bg(p.card)
            .text_color(p.muted_foreground)
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(11.2))
            .when(inst.connection != Connection::Live, |el| el.opacity(0.7))
            .child(initials(&inst.name));
        let name = inst.name.clone();
        let item = self.rail_item(
            id.clone(),
            active,
            false,
            0,
            Nav::Instance { key: inst.key.clone() },
            face,
            &name,
            40.0,
            window,
            cx,
        );
        motion::rise(
            div()
                .relative()
                .child(item)
                .child(
                    div()
                        .absolute()
                        .left(px(RAIL / 2.0 + 18.0 - 10.8))
                        .top(px(40.0 - 2.0 - 10.8))
                        .child(conn_dot(inst.connection, &p)),
                )
                // Unread conversations there: the web's small red count at the chip's top right.
                .when(inst.dm_unread > 0, |el| {
                    let ring = crate::ui::theme::mix(p.background, gpui_kit::rgb(0x000000), 0.25);
                    el.child(
                        div()
                            .absolute()
                            .left(px(RAIL / 2.0 + 18.0 - 10.0))
                            .top(px(-7.0))
                            .h(px(22.0))
                            .min_w(px(22.0))
                            .px(px(7.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border(px(3.0))
                            .border_color(ring)
                            .bg(p.destructive)
                            .text_color(gpui_kit::white())
                            .text_size(px(9.6))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(if inst.dm_unread > 99 { "99+".to_owned() } else { inst.dm_unread.to_string() }),
                    )
                }),
            SharedString::from(format!("{id}|in-{n}")),
            Duration::from_millis(30 * n as u64),
            8.0,
        )
    }

    // ───────────────────────── Arranging ─────────────────────────

    /// The layout an instance's rail shows now.
    fn rail_layout_of(&self, key: &str) -> RailLayout {
        self.core.shared.read(|s| {
            s.instance(key)
                .map(|i| {
                    let ids: Vec<String> = i.servers.iter().map(|sv| sv.id.clone()).collect();
                    rail_layout(i.rail.as_deref(), &ids)
                })
                .unwrap_or_default()
        })
    }

    fn rail_drag_moved(&mut self, key: &str, event: &DragMoveEvent<RailDrag>, cx: &mut Context<Self>) {
        let drag = event.drag(cx).clone();
        if drag.key != key {
            return;
        }
        self.rail.moved_at = Some(std::time::Instant::now());
        let x = f32::from(event.event.position.x - event.bounds.left());
        let py = f32::from(event.event.position.y - event.bounds.top());
        let layout = self.rail_layout_of(key);
        if self.rail.held.as_ref().is_none_or(|(k, h, _)| *k != key || h.id != drag.id) {
            let Some(slots) = self.rail.slots.get(key) else { return };
            let Some(held) = Held::new(slots, &drag.id, drag.folder) else { return };
            self.rail.held = Some((key.to_owned(), held, None));
        }
        let Some((_, held, shown)) = &self.rail.held else { return };
        // Off to the side of the rail, it goes back where it was.
        let found = if (-24.0..RAIL + 40.0).contains(&x) { Some(held.mark(&layout, shown.as_ref(), py)) } else { None };
        if found.as_ref().map(|m| &m.key) != shown.as_ref().map(|m| &m.key) {
            let over = found.as_ref().is_some_and(|m| m.ring.is_some());
            if let Some((_, _, shown)) = &mut self.rail.held {
                *shown = found;
            }
            cx.set_global(RailOver(over));
            cx.notify();
        }
    }

    fn rail_dropped(&mut self, key: &str, drag: &RailDrag, cx: &mut Context<Self>) {
        let held = self.rail.held.take();
        cx.set_global(RailOver(false));
        cx.notify();
        let Some((k, _, Some(mark))) = held else { return };
        if k != key || drag.key != key {
            return;
        }
        let Some(drop) = mark.drop else { return };
        let layout = self.rail_layout_of(key);
        let next = rail::move_rail(&layout, &drop, rail::folder_id);
        if next == layout {
            return;
        }
        let what = if matches!(drop, RailDrop::Combine { .. }) { "rail.folder_create" } else { "rail.arrange" };
        // Everything lands where it shows: the slides start over from the new places.
        self.rail.settled += 1;
        self.arrange_rail(key, next, what, cx);
    }

    /// Shows a new arrangement at once and sends it; put back if the instance says no.
    fn arrange_rail(&mut self, key: &str, next: RailLayout, what: &'static str, cx: &mut Context<Self>) {
        crate::core::reports::used(what);
        self.core.shared.instance(key, |i| i.rail = Some(next.clone()));
        let (core, key) = (self.core.clone(), key.to_owned());
        self.run(cx, async move { core.arrange_servers(&key, next).await }, |this, result, cx| {
            if let Err(err) = result {
                this.toast("circle-alert", err.message, String::new(), None, None, cx);
            }
            cx.notify();
        });
    }

    /// A folder's right-click menu: open or close it, rename and recolor it, or dissolve it.
    pub(crate) fn rail_folder_items(&self, key: &str, folder: &str) -> Built {
        let (k, f) = (key.to_owned(), folder.to_owned());
        let toggle = Item::act(
            t("workspace.rail.folder.openClose"),
            "folder",
            run(move |this, _, cx| {
                let open = this.core.folder_open(&k, &f);
                this.core.set_folder_open(&k, &f, !open);
                this.prefs = this.core.prefs();
                cx.notify();
            }),
        );
        let (k, f) = (key.to_owned(), folder.to_owned());
        let edit = Item::act(
            t("workspace.rail.folder.edit"),
            "palette",
            run(move |this, window, cx| this.edit_folder(&k, &f, window, cx)),
        );
        let (k, f) = (key.to_owned(), folder.to_owned());
        let dissolve = Item::act(
            t("workspace.rail.folder.dissolve"),
            "trash",
            run(move |this, _, cx| {
                let next = rail::edit_folder(&this.rail_layout_of(&k), &f, None);
                this.arrange_rail(&k, next, "rail.folder_dissolve", cx);
            }),
        )
        .danger();
        Built::of(vec![vec![toggle, edit], vec![dissolve]])
    }

    /// For a server's right-click menu: put it in a new folder, or take it out of its own.
    pub(crate) fn rail_server_items(&self, key: &str, server: &str) -> Vec<Item> {
        if !self.core.shared.read(|s| s.instance(key).is_some_and(|i| i.me.is_some())) {
            return Vec::new();
        }
        let layout = self.rail_layout_of(key);
        let inside =
            layout.iter().position(|e| matches!(e, RailEntry::Folder(f) if f.servers.iter().any(|s| s == server)));
        let (k, s) = (key.to_owned(), server.to_owned());
        match inside {
            Some(at) => {
                let after = layout.get(at + 1).map(|e| key_of(e).to_owned());
                vec![Item::act(
                    t("workspace.rail.takeOut"),
                    "folder-minus",
                    run(move |this, _, cx| {
                        let drop = RailDrop::Server { id: s.clone(), folder: String::new(), before: after.clone() };
                        let next = rail::move_rail(&this.rail_layout_of(&k), &drop, rail::folder_id);
                        this.arrange_rail(&k, next, "rail.arrange", cx);
                    }),
                )]
            }
            None => vec![Item::act(
                t("workspace.rail.newFolder"),
                "folder-plus",
                run(move |this, _, cx| {
                    let next = rail::folder_of(&this.rail_layout_of(&k), &s, rail::folder_id);
                    let made = next.iter().find_map(|e| match e {
                        RailEntry::Folder(f) if f.servers == [s.clone()] => Some(f.id.clone()),
                        _ => None,
                    });
                    if let Some(made) = made {
                        this.core.set_folder_open(&k, &made, true);
                        this.prefs = this.core.prefs();
                    }
                    this.arrange_rail(&k, next, "rail.folder_create", cx);
                }),
            )],
        }
    }

    /// The add button's menu: make a server, browse each instance's, or connect to another.
    pub(crate) fn rail_add_items(&self) -> Built {
        let signed_in: Vec<(String, String)> = self.core.shared.read(|s| {
            s.order
                .iter()
                .filter_map(|k| s.instance(k))
                .filter(|i| i.me.is_some())
                .map(|i| (i.key.clone(), i.name()))
                .collect()
        });
        let here = match &self.nav {
            Nav::Server { key, .. }
            | Nav::Instance { key }
            | Nav::Friends { key }
            | Nav::Home { dm: Some((key, _)) } => Some(key.clone()),
            _ => None,
        };
        let create_in = here
            .filter(|k| signed_in.iter().any(|(s, _)| s == k))
            .or_else(|| signed_in.first().map(|(k, _)| k.clone()));
        let mut items = vec![
            Item::act(
                t("workspace.rail.create"),
                "plus",
                run(move |this, window, cx| {
                    if let Some(key) = create_in.clone() {
                        this.open_dialog(crate::ui::app::Dialog::CreateServer { key }, window, cx);
                    }
                }),
            )
            .disabled(signed_in.is_empty()),
        ];
        let streamer = self.prefs.streamer_mode;
        for (key, name) in signed_in {
            // In streamer mode an instance with no name of its own shows no address.
            let name = if streamer && name == key { "•••".to_owned() } else { name };
            let k = key.clone();
            items.push(Item::act(
                t_with("workspace.rail.browseOn", &[("instance", Arg::Str(&name))]),
                "compass",
                run(move |this, window, cx| this.navigate(Nav::Instance { key: k.clone() }, window, cx)),
            ));
        }
        items.push(Item::act(
            t("workspace.rail.connect"),
            "globe",
            run(|this, window, cx| this.open_connect(true, window, cx)),
        ));
        Built::of(vec![items])
    }

    // ───────────────────────── The folder dialog ─────────────────────────

    fn edit_folder(&mut self, key: &str, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.rail_layout_of(key).into_iter().find_map(|e| match e {
            RailEntry::Folder(f) if f.id == folder => Some(f),
            _ => None,
        }) else {
            return;
        };
        let placeholder = self.core.shared.read(|s| {
            let i = s.instance(key);
            folder_label(&RailFolder { name: String::new(), ..f.clone() }, |id| {
                i.and_then(|i| i.server(id)).map(|s| s.name.clone())
            })
        });
        let name = cx.new(|cx| {
            let mut state = gpui_kit::component::input::InputState::new(window, cx).placeholder(placeholder);
            state.set_value(f.name.clone(), window, cx);
            state
        });
        name.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe_in(
            &name,
            window,
            |this: &mut Self, _, event: &gpui_kit::component::input::InputEvent, _, cx| {
                if let gpui_kit::component::input::InputEvent::PressEnter { .. } = event {
                    this.save_folder(cx);
                }
            },
        );
        self.rail.editing =
            Some(FolderEdit { key: key.to_owned(), folder: folder.to_owned(), name, color: f.color, _sub: sub });
        cx.notify();
    }

    fn save_folder(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.rail.editing.take() else { return };
        let name = edit.name.read(cx).value().trim().to_owned();
        let change = FolderChange { name: Some(name), color: Some(edit.color) };
        let next = rail::edit_folder(&self.rail_layout_of(&edit.key), &edit.folder, Some(change));
        self.arrange_rail(&edit.key, next, "rail.folder_edit", cx);
        cx.notify();
    }

    /// Renames and recolors a folder, with a preview of its tile (the web's `FolderDialog`).
    pub(crate) fn render_folder_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.rail.editing.as_ref()?;
        let p = pal(cx);
        let (key, color) = (edit.key.clone(), edit.color);
        let tiles: Vec<pb::Server> = self.core.shared.read(|s| {
            let Some(i) = s.instance(&key) else { return Vec::new() };
            let ids: Vec<String> = i.servers.iter().map(|sv| sv.id.clone()).collect();
            rail_layout(i.rail.as_deref(), &ids)
                .into_iter()
                .find_map(|e| match e {
                    RailEntry::Folder(f) if f.id == edit.folder => Some(f.servers),
                    _ => None,
                })
                .unwrap_or_default()
                .iter()
                .filter_map(|id| i.server(id).cloned())
                .collect()
        });
        let shown = folder_color(color, &p);
        let mut swatches = div().flex().flex_wrap().gap(px(8.0));
        for c in FOLDER_COLORS {
            let on = c == color;
            let fill = folder_color(c, &p);
            let grow =
                motion::follow(SharedString::from(format!("swatch|{c}")), if on { 1.0 } else { 0.0 }, window, cx);
            // The picked one gets the web's two rings: the card's color, then its own.
            let ring = |inset: f32, color: Hsla| div().absolute().inset(px(-inset)).rounded_full().bg(color);
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("swatch-{c}")))
                    .relative()
                    .size(px(32.0))
                    .cursor_pointer()
                    .when(on, |el| el.child(ring(4.0, fill.into())).child(ring(2.0, p.card.into())))
                    .child(ring(0.0, fill.into()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(edit) = &mut this.rail.editing {
                            edit.color = c;
                        }
                        cx.notify();
                    }))
                    .child(
                        div().absolute().inset_0().flex().items_center().justify_center().child(
                            icon("check")
                                .size(px(16.0 * (0.4 + 0.6 * grow)))
                                .text_color(gpui_kit::white())
                                .opacity(grow.clamp(0.0, 1.0)),
                        ),
                    ),
            );
        }
        let close = div()
            .id("folder-close")
            .absolute()
            .top(px(16.0))
            .right(px(16.0))
            .size(px(32.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover({
                let (bg, fg) = (p.muted, p.foreground);
                move |s| s.bg(bg).text_color(fg)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.rail.editing = None;
                cx.notify();
            }))
            .child(icon("x").size(px(16.0)));
        let card = div()
            .id("folder-dialog")
            .relative()
            .w(px(448.0))
            .p(px(24.0))
            .rounded(crate::ui::theme::radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow_2xl()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .mb(px(20.0))
                    .pr(px(32.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("workspace.rail.folder.title")),
                    )
                    .child(
                        div()
                            .mt(px(4.0))
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("workspace.rail.folder.about")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.0))
                            // The web's preview: the tile drawn 1.3 times over a 48px place with 0.4rem around.
                            .child(
                                div()
                                    .size(px(60.8))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(folder_tile(&tiles, false, shown, 1.3, &p).flex_none()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(t("workspace.rail.folder.name")),
                                    )
                                    .child(gpui_kit::component::input::Input::new(&edit.name)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .mb(px(8.0))
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(t("workspace.rail.folder.color")),
                            )
                            .child(swatches),
                    )
                    .child({
                        // The web's default Button: h-9, rounded-md, medium weight.
                        let hover = alpha(p.primary, 0.9);
                        div()
                            .id("folder-save")
                            .h(px(36.0))
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(crate::ui::theme::radius_md())
                            .bg(p.primary)
                            .text_color(p.primary_foreground)
                            .text_size(px(14.0))
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .active(|s| s.opacity(0.9))
                            .on_click(cx.listener(|this, _, _, cx| this.save_folder(cx)))
                            .child(t("workspace.rail.folder.save"))
                    }),
            )
            .child(close);
        Some(
            dialog_scrim("folder-scrim")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.rail.editing = None;
                    cx.notify();
                }))
                .child(motion::rise(card, "folder-dialog-in", Duration::ZERO, 40.0))
                .into_any_element(),
        )
    }
}

/// The web's dialog overlay: the page behind darkened by half.
pub(crate) fn dialog_scrim(id: &'static str) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
        .occlude()
}

/// Moves an element by `by` on a spring (the others making room while one is held).
fn slid<E: IntoElement + gpui_kit::Styled + 'static>(
    el: E,
    id: &str,
    by: f32,
    settled: u64,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::Div {
    let at = motion::follow(SharedString::from(format!("{id}|slide|{settled}")), by, window, cx);
    div().relative().top(px(at)).child(el)
}

/// The name beside something on the rail, drawn over everything so the rail's scrolling doesn't clip it.
fn tooltip(id: &str, name: &str, size: f32, p: &Palette) -> impl IntoElement {
    div().absolute().left(px(RAIL + 2.0)).top(px(size / 2.0 - 15.0)).child(gpui_kit::deferred(
        gpui_kit::anchored().child(motion::slide_in(
            div()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(corner(8.0))
                .bg(p.card)
                .border_1()
                .border_color(p.border)
                .shadow_md()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .child(name.to_owned()),
            SharedString::from(format!("{id}|tip")),
            -6.0,
        )),
    ))
}

/// A divider between groups: the web's `my-1 h-0.5 w-8` line in the border color.
fn divider(p: &Palette) -> impl IntoElement {
    div().w(px(32.0)).h(px(2.0)).my(px(4.0)).flex_none().rounded_full().bg(p.border)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str) -> RailEntry {
        RailEntry::Server(id.into())
    }

    fn f(id: &str, servers: &[&str]) -> RailEntry {
        RailEntry::Folder(RailFolder {
            id: id.into(),
            name: String::new(),
            color: 0,
            servers: servers.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn places_follow_the_layout_and_open_folders() {
        let layout = vec![s("a"), f("g", &["b", "c"]), s("d")];
        let closed = slots(&layout, |_| false);
        let icons: Vec<(&str, f32)> =
            closed.iter().filter(|s| s.kind != SlotKind::Unit).map(|s| (s.id.as_str(), s.top)).collect();
        assert_eq!(icons, [("a", 0.0), ("g", 56.0), ("d", 112.0)]);
        let open = slots(&layout, |_| true);
        let unit = open.iter().find(|s| s.kind == SlotKind::Unit && s.id == "g").unwrap();
        assert_eq!((unit.top, unit.bottom), (56.0, 56.0 + 3.0 * 56.0 - 8.0));
        assert!(open.iter().any(|s| s.id == "c" && s.folder == "g" && s.top == 168.0));
    }

    #[test]
    fn a_held_server_lands_between_onto_or_into() {
        let layout = vec![s("a"), s("b"), f("g", &["c"]), s("d")];
        let all = slots(&layout, |_| false);
        let held = Held::new(&all, "d", false).unwrap();
        // Over the top quarter of "a": before it.
        let m = held.mark(&layout, None, 2.0);
        assert_eq!(m.drop, Some(RailDrop::Server { id: "d".into(), folder: String::new(), before: Some("a".into()) }));
        // Over the middle of "b": a folder of the two.
        let m = held.mark(&layout, None, 56.0 + 24.0);
        assert_eq!(m.drop, Some(RailDrop::Combine { id: "d".into(), with: "b".into() }));
        assert_eq!(m.ring.as_deref(), Some("b"));
        // Over a closed folder: into it.
        let m = held.mark(&layout, None, 112.0 + 30.0);
        assert_eq!(m.drop, Some(RailDrop::Server { id: "d".into(), folder: "g".into(), before: None }));
        // Below everything: last.
        let m = held.mark(&layout, None, 400.0);
        assert_eq!(m.drop, Some(RailDrop::Server { id: "d".into(), folder: String::new(), before: None }));
    }

    #[test]
    fn the_others_slide_to_open_the_place() {
        let layout = vec![s("a"), s("b"), s("c")];
        let all = slots(&layout, |_| false);
        let held = Held::new(&all, "a", false).unwrap();
        // "a" picked up: "b" and "c" move up into its place, then back down past where it'd go.
        assert_eq!(held.shift(0, 0), 0.0);
        assert_eq!(held.shift(0, 1), -56.0);
        assert_eq!(held.shift(1, 1), 0.0);
        let folder = Held::new(&slots(&[f("g", &["x"]), s("b")], |_| false), "g", true).unwrap();
        let drop = folder.mark(&[f("g", &["x"]), s("b")], None, 100.0).drop;
        assert_eq!(drop, Some(RailDrop::Folder { id: "g".into(), before: None }));
    }
}
