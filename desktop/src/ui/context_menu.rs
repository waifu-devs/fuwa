//! Right-click menus on messages, people, channels, categories, servers and
//! conversations, like the web app's `ContextMenu.tsx` (docs/context-menus.md).
//! One at a time: it opens at the pointer and stays in the window, Escape, a
//! click elsewhere, scrolling or resizing closes it, and the keyboard walks it
//! (arrows, Enter, Left and Right for submenus, a letter to jump). What each
//! menu holds is in `menu_items.rs`.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, Keystroke, MouseButton,
    ParentElement as _, Pixels, Point, SharedString, Size, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};

use crate::ui::app::FuwaApp;
use crate::ui::chat::Msg;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_sm, radius_xl};
use crate::ui::widgets::{icon, pal};

/// What an item does, given the app.
pub(crate) type Run = Rc<dyn Fn(&mut FuwaApp, &mut Window, &mut Context<FuwaApp>)>;

pub(crate) fn run(f: impl Fn(&mut FuwaApp, &mut Window, &mut Context<FuwaApp>) + 'static) -> Run {
    Rc::new(f)
}

/// What was right-clicked.
#[derive(Clone)]
pub(crate) enum MenuOf {
    /// A message in the open channel or conversation; `thread` when it's in the
    /// open thread's panel, `picture` the file right-clicked in it.
    Message {
        msg: Rc<Msg>,
        thread: Option<String>,
        picture: Option<usize>,
        selection: String,
    },
    /// Someone's name or picture, in a server or not.
    Member {
        key: String,
        server: Option<String>,
        user_id: String,
    },
    Channel {
        key: String,
        server: String,
        channel: String,
    },
    Category {
        key: String,
        server: String,
        category: String,
    },
    Server {
        key: String,
        server: String,
    },
    /// The menu under a server's name at the top of the sidebar (the web's
    /// server dropdown), which isn't the same as right-clicking its icon.
    ServerHeader {
        key: String,
        server: String,
    },
    Dm {
        key: String,
        conversation: String,
    },
    /// A server you applied to, waiting in the rail (`ui::join`).
    Applied {
        key: String,
        server: String,
    },
    /// A live tile's "…" menu (`live_tiles.rs`), by the tile's id.
    LiveTile {
        key: String,
        server: String,
        tile: String,
    },
    /// A folder on the rail (`rail.rs`).
    RailFolder {
        key: String,
        folder: String,
    },
    /// The rail's add button.
    RailAdd,
    /// The message box (the web's composer menu).
    Composer,
}

impl MenuOf {
    /// What stays lit while its menu is open: a message's id, or a row's.
    pub(crate) fn lit(&self) -> String {
        match self {
            MenuOf::Message { msg, .. } => format!("msg|{}", msg.id),
            MenuOf::Member { user_id, .. } => format!("member|{user_id}"),
            MenuOf::Channel { channel, .. } => format!("row|{channel}"),
            MenuOf::Category { category, .. } => format!("cat|{category}"),
            MenuOf::Server { key, server } => format!("s|{key}|{server}"),
            MenuOf::ServerHeader { key, server } => format!("s-head|{key}|{server}"),
            MenuOf::Dm { key, conversation } => format!("dm|{key}|{conversation}"),
            MenuOf::Applied { key, server } => format!("applied|{key}|{server}"),
            MenuOf::LiveTile { key, server, tile } => format!("tile|{key}|{server}|{tile}"),
            MenuOf::RailFolder { key, folder } => format!("f|{key}|{folder}"),
            MenuOf::RailAdd => "rail-add".into(),
            MenuOf::Composer => "composer".into(),
        }
    }
}

pub(crate) enum Kind {
    Act(Run),
    /// Shows whether it's on; a role stays open to pick another.
    Check {
        on: bool,
        /// One of a set; drawn with a tick all the same, as the web's menus draw them.
        #[allow(dead_code)]
        radio: bool,
        keep_open: bool,
        run: Run,
    },
    Sub(Vec<Item>),
}

/// One line of a menu.
pub(crate) struct Item {
    pub label: String,
    pub icon: &'static str,
    pub hint: Option<String>,
    /// A role's color, by its name.
    pub color: Option<Hsla>,
    pub danger: bool,
    pub disabled: bool,
    /// What can't be undone asks first: a title, a line, and the button's word.
    pub confirm: Option<(String, String, String)>,
    pub kind: Kind,
}

impl Item {
    pub(crate) fn act(label: impl Into<String>, icon: &'static str, run: Run) -> Self {
        Self {
            label: label.into(),
            icon,
            hint: None,
            color: None,
            danger: false,
            disabled: false,
            confirm: None,
            kind: Kind::Act(run),
        }
    }

    pub(crate) fn sub(label: impl Into<String>, icon: &'static str, items: Vec<Item>) -> Self {
        Self { kind: Kind::Sub(items), ..Self::act(label, icon, run(|_, _, _| {})) }
    }

    pub(crate) fn check(label: impl Into<String>, on: bool, radio: bool, run: Run) -> Self {
        Self { kind: Kind::Check { on, radio, keep_open: false, run }, ..Self::act(label, "", self::run(|_, _, _| {})) }
    }

    pub(crate) fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        let hint = hint.into();
        self.hint = (!hint.is_empty()).then_some(hint);
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(crate) fn colored(mut self, color: Option<Hsla>) -> Self {
        self.color = color;
        self
    }

    pub(crate) fn keep_open(mut self) -> Self {
        if let Kind::Check { keep_open, .. } = &mut self.kind {
            *keep_open = true;
        }
        self
    }

    pub(crate) fn confirm(mut self, title: String, body: &str, action: &str) -> Self {
        self.confirm = Some((title, body.to_owned(), action.to_owned()));
        self
    }

    fn enabled(&self) -> bool {
        !self.disabled
    }
}

/// A menu's items in order, with a line before each new section.
pub(crate) struct Built {
    pub items: Vec<Item>,
    /// Whether a line comes before each item.
    pub lines: Vec<bool>,
}

impl Built {
    /// Leaves out empty sections.
    pub(crate) fn of(sections: Vec<Vec<Item>>) -> Self {
        let (mut items, mut lines) = (Vec::new(), Vec::new());
        for section in sections.into_iter().filter(|s| !s.is_empty()) {
            let first = !items.is_empty();
            for (n, item) in section.into_iter().enumerate() {
                lines.push(first && n == 0);
                items.push(item);
            }
        }
        Self { items, lines }
    }
}

/// The open menu.
pub(crate) struct ContextMenu {
    pub of: MenuOf,
    at: Point<Pixels>,
    /// The window's size when it opened: resizing closes it.
    size: Size<Pixels>,
    active: Option<usize>,
    /// The item whose submenu is open, and the one lit in it.
    sub: Option<(usize, Option<usize>)>,
    /// The item asking before it runs.
    asking: Option<usize>,
    typed: (String, Instant),
    opened: Instant,
}

const ROW: f32 = 32.0;
const LINE: f32 = 9.0;
const PAD: f32 = 6.0;
/// `w-60`.
const WIDTH: f32 = 240.0;
/// A submenu's `w-56`.
const SUB_WIDTH: f32 = 224.0;

/// Where an item sits from the top of its card.
fn top_of(lines: &[bool], ix: usize) -> f32 {
    PAD + ix as f32 * ROW + lines[..=ix.min(lines.len().saturating_sub(1))].iter().filter(|l| **l).count() as f32 * LINE
}

/// The next item that can be picked, `by` steps from `from`, round the end.
fn step(items: &[Item], from: Option<usize>, by: isize) -> Option<usize> {
    let n = items.len() as isize;
    if n == 0 {
        return None;
    }
    let mut at = from.map_or(if by > 0 { -1 } else { n }, |a| a as isize);
    for _ in 0..n {
        at = (at + by).rem_euclid(n);
        if items[at as usize].enabled() {
            return Some(at as usize);
        }
    }
    None
}

/// The next item after `from` whose label starts with `typed`.
fn jump(items: &[Item], from: Option<usize>, typed: &str) -> Option<usize> {
    let n = items.len();
    let start = from.map_or(0, |a| a + 1);
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&ix| items[ix].enabled() && items[ix].label.to_lowercase().starts_with(&typed.to_lowercase()))
}

impl FuwaApp {
    /// Opens a menu for what was right-clicked, at `at`; nothing when it would be empty.
    pub(crate) fn open_context_menu(
        &mut self,
        of: MenuOf,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.context_items(&of, cx).items.is_empty() {
            return;
        }
        self.menu = None;
        self.context = Some(ContextMenu {
            of,
            at,
            size: window.viewport_size(),
            active: None,
            sub: None,
            asking: None,
            typed: (String::new(), Instant::now()),
            opened: Instant::now(),
        });
        cx.notify();
    }

    /// For Shift+F10 and the Menu key: what the pointer is over, or the open channel or conversation.
    pub(crate) fn open_context_menu_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let of = self.hover_target.clone().or_else(|| match self.target()? {
            crate::ui::app::Target::Channel { key, server, channel } => Some(MenuOf::Channel { key, server, channel }),
            crate::ui::app::Target::Dm { key, conversation } => Some(MenuOf::Dm { key, conversation }),
            _ => None,
        });
        if let Some(of) = of {
            let at = window.mouse_position();
            let first = step(&self.context_items(&of, cx).items, None, 1);
            self.open_context_menu(of, at, window, cx);
            if let Some(menu) = &mut self.context {
                menu.active = first;
            }
        }
    }

    /// Right-clicking a row the app draws itself opens `of`'s menu.
    pub(crate) fn right_click(
        &self,
        of: MenuOf,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui_kit::MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
            cx.stop_propagation();
            this.open_context_menu(of.clone(), ev.position, window, cx);
        })
    }

    /// Notes what the pointer is over, for Shift+F10 and the Menu key.
    pub(crate) fn set_hover_target(&mut self, of: MenuOf, hovered: bool) {
        if hovered {
            self.hover_target = Some(of);
        } else if self.hover_target.as_ref().is_some_and(|h| h.lit() == of.lit()) {
            self.hover_target = None;
        }
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.context.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    /// Picks an item: runs it, opens its submenu, or asks first.
    fn pick_item(
        &mut self,
        ix: usize,
        sub: Option<usize>,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = &self.context else { return };
        let mut built = self.context_items(&menu.of.clone(), cx);
        let Some(item) = built.items.get_mut(ix) else { return };
        if item.disabled {
            return;
        }
        let item = match (&mut item.kind, sub) {
            (Kind::Sub(items), Some(s)) if s < items.len() => items.swap_remove(s),
            (Kind::Sub(items), None) => {
                let first = step(items, None, 1);
                if let Some(menu) = &mut self.context {
                    menu.sub = Some((ix, first));
                }
                cx.notify();
                return;
            }
            _ => std::mem::replace(item, Item::act("", "", run(|_, _, _| {}))),
        };
        if item.disabled {
            return;
        }
        if item.confirm.is_some() && !confirmed {
            if let Some(menu) = &mut self.context {
                menu.asking = Some(ix);
                menu.sub = None;
            }
            cx.notify();
            return;
        }
        match item.kind {
            Kind::Act(f) => {
                self.context = None;
                f(self, window, cx);
            }
            Kind::Check { keep_open, run, .. } => {
                if !keep_open {
                    self.context = None;
                }
                run(self, window, cx);
            }
            Kind::Sub(_) => {}
        }
        cx.notify();
    }

    /// Keys while a menu is open. Every key goes to it, so typing doesn't land
    /// in a text box under it; a shortcut with Ctrl or Alt closes it and goes on.
    pub(crate) fn context_menu_keys(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &self.context else { return false };
        let m = &key.modifiers;
        if m.control || m.alt || m.platform {
            self.close_context_menu(cx);
            return false;
        }
        let of = menu.of.clone();
        let (active, sub, asking) = (menu.active, menu.sub, menu.asking);
        let built = self.context_items(&of, cx);
        if let Some(ix) = asking {
            match key.key.as_str() {
                "escape" => {
                    if let Some(menu) = &mut self.context {
                        menu.asking = None;
                    }
                }
                "enter" => self.pick_item(ix, None, true, window, cx),
                _ => {}
            }
            cx.notify();
            return true;
        }
        let in_sub = sub.and_then(|(ix, at)| match &built.items.get(ix)?.kind {
            Kind::Sub(items) => Some((ix, at, items)),
            _ => None,
        });
        let Some(menu) = &mut self.context else { return false };
        match (key.key.as_str(), in_sub) {
            ("escape" | "left", Some(_)) => menu.sub = None,
            ("escape", None) => self.context = None,
            ("down" | "up", Some((ix, at, items))) => {
                menu.sub = Some((ix, step(items, at, if key.key == "down" { 1 } else { -1 })));
            }
            ("down" | "up", None) => menu.active = step(&built.items, active, if key.key == "down" { 1 } else { -1 }),
            ("home", None) => menu.active = step(&built.items, None, 1),
            ("end", None) => menu.active = step(&built.items, None, -1),
            ("enter" | "space", Some((ix, Some(at), _))) => self.pick_item(ix, Some(at), false, window, cx),
            ("enter" | "space" | "right", None) => {
                if let Some(ix) = active {
                    let is_sub = matches!(built.items[ix].kind, Kind::Sub(_));
                    if key.key != "right" || is_sub {
                        self.pick_item(ix, None, false, window, cx);
                    }
                }
            }
            (other, at) => {
                // Typing jumps to an item; letters typed together find a longer label.
                let Some(ch) = key.key_char.as_deref().filter(|c| c.chars().count() == 1 && !c.trim().is_empty())
                else {
                    return true;
                };
                let _ = other;
                let (typed, at_time) = &mut menu.typed;
                if at_time.elapsed() > Duration::from_millis(700) {
                    typed.clear();
                }
                typed.push_str(ch);
                *at_time = Instant::now();
                let typed = typed.clone();
                match at {
                    Some((ix, at, items)) => {
                        let found = jump(items, at, &typed).or_else(|| jump(items, None, ch));
                        if found.is_some() {
                            menu.sub = Some((ix, found));
                        }
                    }
                    None => {
                        let found = jump(&built.items, active, &typed).or_else(|| jump(&built.items, None, ch));
                        if found.is_some() {
                            menu.active = found;
                        }
                    }
                }
            }
        }
        cx.notify();
        true
    }

    /// The open menu over everything, or nothing.
    pub(crate) fn render_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.context.as_ref()?;
        if window.viewport_size() != menu.size {
            self.context = None;
            return None;
        }
        let p = pal(cx);
        let (of, at, active, sub, asking, opened) =
            (menu.of.clone(), menu.at, menu.active, menu.sub, menu.asking, menu.opened);
        let built = self.context_items(&of, cx);
        if built.items.is_empty() {
            self.context = None;
            return None;
        }
        let id = format!("{opened:?}");
        // What can't be undone asks first, in a dialog of its own, as the web's menus do.
        if let Some((ix, item)) = asking.and_then(|ix| built.items.get(ix).map(|i| (ix, i))) {
            let (title, body, action) = item.confirm.clone().unwrap_or_default();
            let panel = self.confirm_panel(
                title,
                body,
                action,
                &p,
                cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)),
                cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)),
                cx.listener(move |this, _, window, cx| this.pick_item(ix, None, true, window, cx)),
            );
            return Some(crate::ui::overlay::dialog_layer(
                &format!("confirm-{id}"),
                panel,
                &p,
                cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)),
            ));
        }
        let body = self.items_body(&built, active, sub, &id, at, window, &p, cx);
        let layer = div()
            .id("context-away")
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)))
            .on_mouse_down(MouseButton::Middle, cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| _ = this.close_context_menu(cx)))
            .child(
                gpui_kit::anchored()
                    .position(at + gpui_kit::point(px(0.0), px(2.0)))
                    .snap_to_window_with_margin(gpui_kit::Edges::all(px(8.0)))
                    .child(
                        div()
                            .id("context-menu")
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                            .child(pop(menu_card(&p).child(body), SharedString::from(format!("ctx|{id}")))),
                    ),
            );
        Some(layer.into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn items_body(
        &self,
        built: &Built,
        active: Option<usize>,
        sub: Option<(usize, Option<usize>)>,
        id: &str,
        at: Point<Pixels>,
        window: &mut Window,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lit = active.or(sub.map(|(ix, _)| ix));
        let mut list = div().relative().w(px(WIDTH - 2.0)).py(px(PAD)).px(px(PAD)).flex().flex_col();
        // The highlight glides from item to item.
        if let Some(ix) = lit {
            let y = motion::follow(SharedString::from(format!("ctx-hl|{id}")), top_of(&built.lines, ix), window, cx);
            let danger = built.items[ix].danger;
            list = list.child(
                div()
                    .absolute()
                    .left(px(PAD))
                    .right(px(PAD))
                    .top(px(y))
                    .h(px(ROW))
                    .rounded(radius_sm())
                    .bg(if danger { alpha(p.destructive, 0.1) } else { p.accent.into() }),
            );
        }
        for (ix, item) in built.items.iter().enumerate() {
            if built.lines[ix] {
                list = list.child(div().h(px(1.0)).my(px(4.0)).mx(px(4.0)).bg(p.border));
            }
            let is_sub = matches!(item.kind, Kind::Sub(_));
            let open = sub.is_some_and(|(o, _)| o == ix);
            let row =
                row(item, false, open, p).id(SharedString::from(format!("ctx-item|{ix}"))).when(item.enabled(), |el| {
                    el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if let Some(menu) = &mut this.context
                            && *hovered
                            && menu.active != Some(ix)
                        {
                            menu.active = Some(ix);
                            // Pointing at another item closes the open submenu; at one with a submenu, opens it.
                            if is_sub {
                                menu.sub = Some((ix, None));
                            } else if menu.sub.is_some_and(|(s, _)| s != ix) {
                                menu.sub = None;
                            }
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_item(ix, None, false, window, cx)))
                });
            // Items enter one after another, the first ten.
            let delay = Duration::from_millis(12 * ix.min(10) as u64);
            list =
                list.child(motion::rise(div().child(row), SharedString::from(format!("ctx-in|{id}|{ix}")), delay, 4.0));
        }
        // An open submenu, beside its item; on the left when there's no room on the right.
        if let Some((ix, sub_active)) = sub
            && let Some(Kind::Sub(items)) = built.items.get(ix).map(|i| &i.kind)
        {
            let room = f32::from(window.viewport_size().width) - f32::from(at.x) - WIDTH;
            let left = room > SUB_WIDTH + 12.0;
            let mut col = div()
                .id("ctx-sub-list")
                .w(px(SUB_WIDTH - 2.0))
                .max_h(px(384.0))
                .overflow_y_scroll()
                .py(px(PAD))
                .px(px(PAD))
                .flex()
                .flex_col();
            for (n, item) in items.iter().enumerate() {
                let on = sub_active == Some(n);
                col = col.child(row(item, on, false, p).id(SharedString::from(format!("ctx-sub|{ix}|{n}"))).when(
                    item.enabled(),
                    |el| {
                        el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if let Some(menu) = &mut this.context
                                && *hovered
                            {
                                menu.sub = Some((ix, Some(n)));
                                cx.notify();
                            }
                        }))
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.pick_item(ix, Some(n), false, window, cx)),
                        )
                    },
                ));
            }
            // Level with its item: the card's border where the item's highlight starts.
            let top = top_of(&built.lines, ix) - 1.0;
            list = list.child(
                div()
                    .absolute()
                    .top(px(top))
                    .occlude()
                    // Outside the menu's own box: its clicks mustn't reach the layer that closes it.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .when(left, |el| el.left(px(WIDTH + 3.0)))
                    .when(!left, |el| el.right(px(WIDTH + 3.0)))
                    .child(motion::slide_in(
                        menu_card(p).child(col),
                        SharedString::from(format!("ctx-sub-in|{id}|{ix}")),
                        if left { -8.0 } else { 8.0 },
                    )),
            );
        }
        list.into_any_element()
    }
}

/// An item's line, as the web's dropdown items draw it (`px-2 py-1.5 gap-2
/// text-sm`, a 16px icon in the muted color): checkbox items keep the icon's
/// place for their tick (`pl-8`), and a lit item sits on the accent.
fn row(item: &Item, lit: bool, open: bool, p: &Palette) -> gpui_kit::Div {
    let fg: Hsla = if item.danger { p.destructive.into() } else { p.foreground.into() };
    let check = matches!(item.kind, Kind::Check { .. });
    let on = matches!(item.kind, Kind::Check { on: true, .. });
    let glyph: AnyElement = if check {
        // The tick sits where the icon would be (`left-2 size-3.5`).
        div()
            .size(px(16.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .when(on, |el| el.child(icon("check").size(px(16.0)).text_color(p.foreground)))
            .into_any_element()
    } else if item.icon.is_empty() {
        div().size(px(16.0)).flex_none().into_any_element()
    } else {
        icon(item.icon)
            .size(px(16.0))
            .flex_none()
            .text_color(if item.danger { fg } else { p.muted_foreground.into() })
            .into_any_element()
    };
    div()
        .relative()
        .h(px(ROW))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(radius_sm())
        .text_sm()
        .text_color(fg)
        .when(item.danger, |el| el.font_weight(FontWeight::BOLD))
        .when(lit, |el| el.bg(if item.danger { alpha(p.destructive, 0.1) } else { p.accent.into() }))
        .when(item.disabled, |el| el.opacity(0.5))
        .child(glyph)
        .when_some(item.color.filter(|_| check), |el, c| {
            el.child(div().size(px(10.0)).flex_none().rounded_full().bg(c))
        })
        .child(div().flex_1().min_w_0().truncate().child(item.label.clone()))
        .when_some(item.hint.clone(), |el, hint| {
            el.child(
                div()
                    .flex_none()
                    .max_w(px(108.0))
                    .pl(px(8.0))
                    .truncate()
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(hint),
            )
        })
        .when(matches!(item.kind, Kind::Sub(_)), |el| {
            // The web's chevron turns down while its submenu is open.
            el.child(
                icon(if open { "chevron-down" } else { "chevron-right" })
                    .size(px(16.0))
                    .flex_none()
                    .text_color(p.muted_foreground),
            )
        })
}

/// The web's menu card: the popover color, a border, `rounded-xl` and `shadow-xl`.
fn menu_card(p: &Palette) -> gpui_kit::Div {
    div()
        .bg(p.card)
        .text_color(p.foreground)
        .border_1()
        .border_color(p.border)
        .rounded(radius_xl())
        .shadow(crate::ui::settings_controls::shadow_xl())
}

/// The menu springs open from a little smaller out of its top corner, as it
/// fades in (the web's `zoom-in-95`).
fn pop<E: IntoElement + gpui_kit::Styled + 'static>(el: E, id: SharedString) -> impl IntoElement {
    motion::pop_in(el, id, (0.0, 0.0), 0.95, -6.0)
}

/// Copies text, saying what was copied (never the text: in streamer mode it may be private).
pub(crate) fn copy(this: &mut FuwaApp, text: String, what: &str, cx: &mut Context<FuwaApp>) {
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
    // The web's note: "Copied text", "Copied link"…
    let note = crate::core::i18n::t_with("common.copied", &[("what", crate::core::i18n::Arg::Str(what))]);
    this.toast("copy", note, String::new(), None, None, cx);
}

/// What right-clicking opens, for elements drawn outside the app's own
/// `Context` (message rows, the members list): `of` is built when it's clicked.
pub(crate) fn on_right_click(
    this: gpui_kit::WeakEntity<FuwaApp>,
    of: impl Fn(&mut Window, &mut App) -> Option<MenuOf> + 'static,
) -> impl Fn(&gpui_kit::MouseDownEvent, &mut Window, &mut App) + 'static {
    move |ev, window, cx| {
        let Some(of) = of(window, cx) else { return };
        cx.stop_propagation();
        let at = ev.position;
        let _ = this.update(cx, |this, cx| this.open_context_menu(of, at, window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, disabled: bool) -> Item {
        Item::act(label, "x", run(|_, _, _| {})).disabled(disabled)
    }

    #[test]
    fn keys_skip_what_cant_be_picked() {
        let items = vec![item("Mark as read", true), item("Invite people", false), item("Copy link", false)];
        assert_eq!(step(&items, None, 1), Some(1));
        assert_eq!(step(&items, Some(2), 1), Some(1));
        assert_eq!(step(&items, Some(1), -1), Some(2));
        assert_eq!(jump(&items, None, "c"), Some(2));
        assert_eq!(jump(&items, Some(2), "i"), Some(1));
        assert_eq!(jump(&items, None, "m"), None);
    }

    #[test]
    fn sections_get_a_line_between_them() {
        let built = Built::of(vec![vec![item("a", false)], vec![], vec![item("b", false), item("c", false)]]);
        assert_eq!(built.lines, vec![false, true, false]);
        assert_eq!(top_of(&built.lines, 0), PAD);
        assert_eq!(top_of(&built.lines, 1), PAD + ROW + LINE);
        assert_eq!(top_of(&built.lines, 2), PAD + 2.0 * ROW + LINE);
    }
}
