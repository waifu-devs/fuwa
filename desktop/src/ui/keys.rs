//! The keyboard, beyond typing: the shortcuts in `core/keybinds.rs` and what
//! they do (the web's `components/Shortcuts.tsx`), the shortcut sheet, and
//! the quick switcher that finds a server or channel by a few letters.
//!
//! Shortcuts are read on the way down to whatever has focus, so a text box
//! never sees the ones it shouldn't; the Keyboard page records new ones
//! through here too.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement,
    KeyDownEvent, Keystroke, ParentElement as _, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledText, Subscription, Window, div, px,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::keybinds::{self, Action, COMPOSER_KEYS, Group};
use crate::pb;
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, corner, mix, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{icon, pal, server_icon};

/// The combo a key press makes, in the web's words; None for a bare modifier.
pub fn combo_of(key: &Keystroke) -> Option<String> {
    let name = keybinds::key_name(&key.key)?;
    let m = &key.modifiers;
    let mut parts = keybinds::modifiers(m.control, m.alt, m.shift, m.platform);
    parts.push(&name);
    Some(parts.join("+"))
}

/// The quick switcher: its search box and the row the arrows are on.
pub struct Switcher {
    pub query: Entity<InputState>,
    pub active: usize,
    /// Keeps the picked row in sight when the arrows move past the edge.
    scroll: ScrollHandle,
    _subscription: Subscription,
}

/// One place the switcher can go.
#[derive(Clone)]
struct Item {
    key: String,
    server: String,
    channel: Option<String>,
    name: String,
    /// Where it lives: the server and instance, or just the instance.
    place: String,
    kind: pb::ChannelType,
    unread: u32,
    hits: Vec<usize>,
    score: f32,
}

/// How well `query` matches `text`, letters in order (the web's `lib/fuzzy.ts`):
/// runs and word starts count more. The places of the letters it matched.
pub fn fuzzy(query: &str, text: &str) -> Option<(f32, Vec<usize>)> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some((0.0, Vec::new()));
    }
    let (mut hits, mut score, mut from, mut previous) = (Vec::new(), 0.0f32, 0usize, None::<usize>);
    for ch in &q {
        let at = (from..t.len()).find(|&i| t[i] == *ch)?;
        let word_start = at == 0 || matches!(t[at - 1], ' ' | '-' | '_' | '.' | '/' | '·');
        score +=
            1.0 + if previous.is_some_and(|p| p + 1 == at) { 3.0 } else { 0.0 } + if word_start { 2.0 } else { 0.0 };
        hits.push(at);
        previous = Some(at);
        from = at + 1;
    }
    let (q, t): (String, String) = (q.into_iter().collect(), t.iter().collect());
    if t.starts_with(&q) {
        score += 6.0;
    } else if t.contains(&q) {
        score += 3.0;
    }
    Some((score - t.chars().count() as f32 * 0.02, hits))
}

fn wrap(n: isize, len: usize) -> usize {
    n.rem_euclid(len as isize) as usize
}

impl FuwaApp {
    /// Hands keys a binding caught to the shortcut being changed in settings, if one is listening.
    pub(crate) fn forward_to_recording(&mut self, keys: &str, cx: &mut Context<Self>) -> bool {
        let Some(settings) = self.settings.clone().filter(|s| s.read(cx).recording()) else { return false };
        let Ok(key) = Keystroke::parse(keys) else { return false };
        settings.update(cx, |s, cx| s.record(&key, cx));
        true
    }

    // ───────────────────────── Shortcuts ─────────────────────────

    /// Every key on its way down: recorded for the Keyboard page, or run as a shortcut.
    pub(crate) fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.core.idle.seen();
        if let Some(settings) = self.settings.clone()
            && settings.read(cx).recording()
        {
            settings.update(cx, |s, cx| s.record(&ev.keystroke, cx));
            cx.stop_propagation();
            return;
        }
        if self.switcher_keys(&ev.keystroke, window, cx) || self.sheet_keys(&ev.keystroke, cx) {
            cx.stop_propagation();
            return;
        }
        let Some(combo) = combo_of(&ev.keystroke) else { return };
        let all = keybinds::bindings(&self.prefs.keybinds, &self.prefs.custom_keybinds);
        let Some(action) = all.get(&keybinds::normalize(&combo)).copied() else { return };
        if ev.is_held && !action.repeats {
            cx.stop_propagation();
            return;
        }
        let focused = window.focused(cx);
        let typing = focused.as_ref().is_some_and(|f| *f != self.focus);
        if typing && !action.while_typing {
            return;
        }
        // With a dialog, a menu or a screen over the app, only what opens or closes those works.
        let covered = self.menu.is_some()
            || self.dialog.is_some()
            || self.connect.is_some()
            || self.server_settings.is_some()
            || self.instance_settings.is_some()
            || self.settings.is_some()
            || self.switcher.is_some()
            || self.sheet_open;
        if covered {
            let allowed = match action.id {
                "toggleStreamer" => true,
                "openSettings" => self.settings.is_some(),
                "shortcuts" => self.sheet_open,
                "quickSwitcher" => self.switcher.is_some(),
                _ => false,
            };
            if !allowed {
                return;
            }
        }
        cx.stop_propagation();
        if action.id == "pushToTalk" {
            self.push_to_talk(&ev.keystroke, cx);
            return;
        }
        self.run_shortcut(action, window, cx);
    }

    fn run_shortcut(&mut self, action: &Action, window: &mut Window, cx: &mut Context<Self>) {
        match action.id {
            "quickSwitcher" => {
                if self.switcher.is_some() {
                    self.close_switcher(window, cx);
                } else {
                    self.open_switcher(window, cx);
                }
            }
            "previousServer" => self.step_server(-1, window, cx),
            "nextServer" => self.step_server(1, window, cx),
            "previousChannel" => self.step_channel(-1, window, cx),
            "nextChannel" => self.step_channel(1, window, cx),
            "previousUnread" => self.step_unread(-1, window, cx),
            "nextUnread" => self.step_unread(1, window, cx),
            "markServerRead" => self.mark_server_read(cx),
            "focusComposer" => {
                if self.target().is_some() {
                    self.composer.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            "insertTimestamp" => self.open_time_picker(window, cx),
            "searchServer" => self.focus_search(window, cx),
            "toggleMembers" => self.members_open = !self.members_open,
            // Mute and deafen work in and out of calls, as the web's do (call_parts.rs).
            "toggleMute" => {
                let (mute, deaf) = self.core.selves();
                self.core.set_self_mute(!(mute || deaf));
            }
            "toggleDeafen" => {
                let (_, deaf) = self.core.selves();
                self.core.set_self_deaf(!deaf);
            }
            "toggleRecording" => {
                if let Some(call) = self.core.call() {
                    self.core.set_recording(!call.self_record);
                }
            }
            "openSettings" => {
                if self.settings.is_some() {
                    self.settings = None;
                    self.focus.focus(window, cx);
                } else {
                    self.open_settings(window, cx);
                }
            }
            "shortcuts" => self.sheet_open = !self.sheet_open,
            "toggleStreamer" => {
                let on = !self.prefs.streamer_mode;
                self.core.set_prefs(|p| p.streamer_mode = on);
                self.prefs = self.core.prefs();
                let title = if on { t("chattools.shortcuts.streamerOn") } else { t("chattools.shortcuts.streamerOff") };
                self.toast("eye-off", title, String::new(), None, None, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Every joined server, in the rail's order, as (instance, server).
    fn rail_servers(&self) -> Vec<(String, String)> {
        self.core.shared.read(|s| {
            s.order
                .iter()
                .flat_map(|key| {
                    s.instance(key)
                        .map(|i| i.servers.iter().map(|sv| (key.clone(), sv.id.clone())).collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect()
        })
    }

    /// A server's channels that open, in the sidebar's order.
    fn openable(&self, key: &str, server: &str) -> Vec<pb::Channel> {
        self.core.shared.read(|s| {
            let Some(channels) = s.instance(key).and_then(|i| i.channels.get(server)) else { return Vec::new() };
            let layout = crate::core::arrange::layout_of(channels);
            layout
                .loose
                .iter()
                .chain(layout.categories.iter().flat_map(|(_, ids)| ids))
                .filter_map(|id| channels.iter().find(|c| &c.id == id))
                .filter(|c| {
                    matches!(
                        pb::ChannelType::try_from(c.r#type),
                        Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                    )
                })
                .cloned()
                .collect()
        })
    }

    fn step_server(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.rail_servers();
        if list.is_empty() {
            return;
        }
        let at = match &self.nav {
            Nav::Server { key, server } => list.iter().position(|(k, s)| k == key && s == server),
            _ => None,
        };
        let at = match (at, &self.nav) {
            (Some(at), _) => at as isize,
            // From an instance's page, the next server is its first one.
            (None, Nav::Instance { key }) => match list.iter().position(|(k, _)| k == key) {
                Some(first) if by > 0 => first as isize - 1,
                Some(first) => first as isize,
                None if by > 0 => -1,
                None => 0,
            },
            (None, _) => {
                if by > 0 {
                    -1
                } else {
                    0
                }
            }
        };
        let (key, server) = list[wrap(at + by, list.len())].clone();
        self.navigate(Nav::Server { key, server }, window, cx);
    }

    fn step_channel(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Nav::Server { key, server } = self.nav.clone() else { return };
        let list = self.openable(&key, &server);
        if list.is_empty() {
            return;
        }
        let open = self.channel_in(&key, &server);
        let next = match list.iter().position(|c| Some(&c.id) == open.as_ref()) {
            Some(at) => wrap(at as isize + by, list.len()),
            None if by > 0 => 0,
            None => list.len() - 1,
        };
        self.open_channel(&key, &server, &list[next].id.clone(), window, cx);
    }

    fn step_unread(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let all: Vec<(String, String, String)> = self
            .rail_servers()
            .into_iter()
            .flat_map(|(key, server)| {
                self.openable(&key, &server).into_iter().map(move |c| (key.clone(), server.clone(), c.id))
            })
            .collect();
        if !all.is_empty() {
            let here = match self.target() {
                Some(crate::ui::app::Target::Channel { key, channel, .. }) => {
                    all.iter().position(|(k, _, c)| *k == key && *c == channel)
                }
                _ => None,
            };
            let start = match here {
                Some(at) => at as isize,
                None if by < 0 => 0,
                None => -1,
            };
            let unread = |k: &str, c: &str| {
                self.core.shared.read(|s| s.instance(k).and_then(|i| i.unread.get(c).copied()).unwrap_or(0))
            };
            for n in 1..=all.len() as isize {
                let (key, server, channel) = &all[wrap(start + by * n, all.len())];
                if unread(key, channel) > 0 {
                    let (key, server, channel) = (key.clone(), server.clone(), channel.clone());
                    self.open_channel(&key, &server, &channel, window, cx);
                    return;
                }
            }
        }
        self.toast("check", t("chattools.shortcuts.caughtUp"), String::new(), None, None, cx);
    }

    fn mark_server_read(&mut self, cx: &mut Context<Self>) {
        let Nav::Server { key, server } = self.nav.clone() else { return };
        let (cleared, name) = self.core.shared.update(|s| {
            let Some(i) = s.instances.get_mut(&key) else { return (0, String::new()) };
            let name = i.servers.iter().find(|sv| sv.id == server).map(|sv| sv.name.clone()).unwrap_or_default();
            let ids: Vec<String> =
                i.channels.get(&server).map(|c| c.iter().map(|c| c.id.clone()).collect()).unwrap_or_default();
            let cleared = ids.iter().filter(|id| i.unread.remove(*id).is_some_and(|n| n > 0)).count();
            (cleared, name)
        });
        let title = match (cleared > 0, name.is_empty()) {
            (true, true) => t("chattools.shortcuts.markedReadThis"),
            (false, true) => t("chattools.shortcuts.nothingUnreadThis"),
            (true, false) => t_with("chattools.shortcuts.markedRead", &[("server", Arg::Str(&name))]),
            (false, false) => t_with("chattools.shortcuts.nothingUnread", &[("server", Arg::Str(&name))]),
        };
        self.toast("check", title, String::new(), None, None, cx);
    }

    // ───────────────────────── The quick switcher ─────────────────────────

    pub(crate) fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::core::reports::used("quick_switcher.open");
        self.sheet_open = false;
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("chattools.switcher.placeholder")));
        let subscription = cx.subscribe_in(&query, window, |this: &mut Self, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(s) = &mut this.switcher
            {
                s.active = 0;
                cx.notify();
            }
        });
        query.update(cx, |s, cx| s.focus(window, cx));
        self.switcher = Some(Switcher { query, active: 0, scroll: ScrollHandle::new(), _subscription: subscription });
        cx.notify();
    }

    pub(crate) fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The arrows, Enter and Escape while the switcher is open.
    fn switcher_keys(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(switcher) = &self.switcher else { return false };
        let m = &key.modifiers;
        if m.control || m.alt || m.platform || m.shift {
            return false;
        }
        let count = self.switcher_items(cx).len();
        let active = switcher.active;
        match key.key.as_str() {
            "escape" => self.close_switcher(window, cx),
            "down" | "up" if count > 0 => {
                let by = if key.key == "down" { 1 } else { -1 };
                if let Some(s) = &mut self.switcher {
                    s.active = wrap(active as isize + by, count);
                    // The list's first child is its heading when nothing is typed.
                    let heading = usize::from(s.query.read(cx).value().is_empty());
                    s.scroll.scroll_to_item(s.active + heading);
                }
                cx.notify();
            }
            "enter" => {
                let item = self.switcher_items(cx).into_iter().nth(active);
                if let Some(item) = item {
                    self.pick(item, window, cx);
                }
            }
            _ => return false,
        }
        true
    }

    fn pick(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        match &item.channel {
            Some(channel) => self.open_channel(&item.key, &item.server, channel, window, cx),
            None => self.navigate(Nav::Server { key: item.key, server: item.server }, window, cx),
        }
        if self.target().is_some() {
            self.composer.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// What the switcher shows for what's typed: unread channels first with
    /// nothing typed, else the best matches. `#` looks for channels only, `*` servers only.
    fn switcher_items(&self, cx: &Context<Self>) -> Vec<Item> {
        let Some(switcher) = &self.switcher else { return Vec::new() };
        let typed = switcher.query.read(cx).value().to_string();
        let (mode, q) = match typed.chars().next() {
            Some('#') => (Some('#'), &typed[1..]),
            Some('*') => (Some('*'), &typed[1..]),
            _ => (None, typed.as_str()),
        };
        let here = self.target();
        let streamer = self.prefs.streamer_mode;
        let mut out = Vec::new();
        for (key, server) in self.rail_servers() {
            let (server_name, instance_name) = self.core.shared.read(|s| {
                let i = s.instance(&key);
                let server_name = i
                    .and_then(|i| i.servers.iter().find(|sv| sv.id == server))
                    .map(|sv| sv.name.clone())
                    .unwrap_or_default();
                let node = i.and_then(|i| i.node.as_ref()).map(|n| n.name.clone()).unwrap_or_default();
                (server_name, node)
            });
            let instance_name =
                if streamer || instance_name.is_empty() { t("desktop.switcher.anInstance") } else { instance_name };
            let unread_of = |c: &str| {
                self.core.shared.read(|s| s.instance(&key).and_then(|i| i.unread.get(c).copied()).unwrap_or(0))
            };
            let channels = self.openable(&key, &server);
            if mode != Some('*') {
                for c in &channels {
                    let open_now = matches!(&here, Some(crate::ui::app::Target::Channel { key: k, channel, .. }) if *k == key && *channel == c.id);
                    if open_now && q.is_empty() {
                        continue;
                    }
                    let Some((score, hits)) = fuzzy(q, &format!("{} {server_name}", c.name)) else { continue };
                    let len = c.name.chars().count();
                    let in_name = hits.iter().all(|h| *h < len);
                    let this_server =
                        matches!(&self.nav, Nav::Server { key: k, server: sv } if *k == key && *sv == server);
                    out.push(Item {
                        key: key.clone(),
                        server: server.clone(),
                        channel: Some(c.id.clone()),
                        name: c.name.clone(),
                        place: format!("{server_name} · {instance_name}"),
                        kind: pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text),
                        unread: unread_of(&c.id),
                        hits: hits.into_iter().filter(|h| *h < len).collect(),
                        score: score + if in_name { 4.0 } else { 0.0 } + if this_server { 1.0 } else { 0.0 },
                    });
                }
            }
            if mode != Some('#') {
                let Some((score, hits)) = fuzzy(q, &server_name) else { continue };
                out.push(Item {
                    key: key.clone(),
                    server: server.clone(),
                    channel: None,
                    name: server_name.clone(),
                    place: instance_name,
                    kind: pb::ChannelType::Category,
                    unread: channels.iter().map(|c| unread_of(&c.id)).sum(),
                    hits,
                    score: score + if q.is_empty() { -1.0 } else { 0.0 },
                });
            }
        }
        let cmp = |a: f32, b: f32| b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal);
        if q.is_empty() {
            out.sort_by(|a, b| {
                let ua = a.channel.is_some() && a.unread > 0;
                let ub = b.channel.is_some() && b.unread > 0;
                ub.cmp(&ua).then(b.unread.cmp(&a.unread)).then(cmp(a.score, b.score))
            });
        } else {
            out.sort_by(|a, b| cmp(a.score, b.score).then(b.unread.cmp(&a.unread)));
        }
        out.truncate(40);
        out
    }

    pub(crate) fn render_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let switcher = self.switcher.as_ref()?;
        // `pt-[12vh]` and `max-h-[70vh]`: parts of the window's height.
        let tall = f32::from(window.viewport_size().height);
        let p = pal(cx);
        let typed = switcher.query.read(cx).value().to_string();
        let active = switcher.active;
        let items = self.switcher_items(cx);
        // `p-2`, scrolling inside the panel's `max-h-[70vh]`.
        let mut list = div()
            .id("switcher-list")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(8.0))
            .overflow_y_scroll()
            .track_scroll(&switcher.scroll);
        let titled = typed.is_empty() && !items.is_empty();
        if !items.is_empty() {
            // The lit row's fill glides from row to row (the web's `layoutId="switcher-active"`):
            // the list's 8px, the title's 26 and 56 a row.
            let top = 8.0 + if titled { 26.0 } else { 0.0 } + active.min(items.len() - 1) as f32 * SWITCH_ROW;
            let top = motion::follow(
                SharedString::from(format!("switcher-lit|{}", switcher.query.entity_id())),
                top,
                window,
                cx,
            );
            list = list.child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .right(px(8.0))
                    .top(px(top))
                    .h(px(SWITCH_ROW))
                    .rounded(radius_xl())
                    .bg(alpha(p.primary, 0.12)),
            );
        }
        if titled {
            let first_unread = items[0].channel.is_some() && items[0].unread > 0;
            list = list.child(
                div()
                    .flex_shrink_0()
                    .px(px(8.0))
                    .pt(px(4.0))
                    .pb(px(6.0))
                    .text_size(px(11.2))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(
                        if first_unread { t("chattools.switcher.unreadFirst") } else { t("chattools.switcher.jumpTo") }
                            .to_uppercase(),
                        WIDE,
                    )),
            );
        }
        if items.is_empty() {
            let text = if typed.is_empty() {
                t("chattools.switcher.empty")
            } else {
                t_with("chattools.switcher.nothingCalled", &[("query", Arg::Str(&typed))])
            };
            list = list.child(motion::rise(
                div()
                    .px(px(12.0))
                    .py(px(32.0))
                    .text_center()
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(text),
                "switcher-empty",
                Duration::ZERO,
                6.0,
            ));
        }
        for (n, item) in items.into_iter().enumerate() {
            list = list.child(self.switcher_row(item, n, n == active, &p, window, cx));
        }
        let footer = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(16.0))
            .gap_y(px(4.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .text_size(px(11.2))
            .line_height(px(16.0))
            .text_color(p.muted_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("arrow-up").size(px(12.0)))
                    .child(icon("arrow-down").size(px(12.0)))
                    .child(t("chattools.switcher.move")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("corner-down-left").size(px(12.0)))
                    .child(t("chattools.switcher.go")),
            )
            .child(marked(t_with("chattools.switcher.channelsOnly", &[("mark", Arg::Str("#"))]), "#", &p))
            .child(marked(t_with("chattools.switcher.serversOnly", &[("mark", Arg::Str("*"))]), "*", &p));
        // `max-w-xl rounded-2xl border bg-popover shadow-2xl`, at most 70% of the window tall.
        let panel =
            div()
                .id("switcher")
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .w_full()
                .max_w(px(576.0))
                .max_h(px(tall * 0.7))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .text_color(p.foreground)
                .shadow(crate::ui::overlay::shadow_2xl())
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(57.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(p.border)
                        .text_lg()
                        .child(Input::new(&switcher.query).appearance(false).text_size(px(18.0)).prefix(
                            div().mr(px(4.0)).child(icon("search").size(px(20.0)).text_color(p.muted_foreground)),
                        )),
                )
                .child(list)
                .child(footer);
        Some(
            div()
                .id("switcher-scrim")
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(tall * 0.12))
                .px(px(12.0))
                .occlude()
                .on_click(cx.listener(|this, _, window, cx| this.close_switcher(window, cx)))
                // The web's `bg-black/45 backdrop-blur-[3px]`, fading in.
                .child(motion::fade_in(
                    div().absolute().inset_0().bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.45)).backdrop_blur(px(3.0)),
                    "switcher-scrim-in",
                    Duration::from_millis(180),
                ))
                .child(motion::pop_in(
                    div().w_full().max_w(px(576.0)).flex().flex_col().child(panel),
                    "switcher-panel",
                    (0.5, 0.5),
                    0.94,
                    -12.0,
                ))
                .into_any_element(),
        )
    }

    /// One place to go: an icon tile (or the server's icon), its name with the
    /// letters that matched, where it is under it, an unread count, and ⏎ on the lit one.
    fn switcher_row(
        &self,
        item: Item,
        n: usize,
        active: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lead: AnyElement = match (&item.channel, item.kind) {
            (None, _) => {
                let server = self
                    .core
                    .shared
                    .read(|s| s.instance(&item.key).and_then(|i| i.server(&item.server)).cloned())
                    .unwrap_or_default();
                server_icon(&server, 32.0, 16.0, p).text_size(px(10.4)).into_any_element()
            }
            (Some(_), kind) => div()
                .size(px(32.0))
                .flex_none()
                .rounded(radius_lg())
                .flex()
                .items_center()
                .justify_center()
                .bg(p.muted)
                .child(
                    icon(match kind {
                        pb::ChannelType::Announcement => "megaphone",
                        pb::ChannelType::Voice => "volume-2",
                        pb::ChannelType::Secure => "shield-check",
                        _ => "hash",
                    })
                    .size(px(16.0)),
                )
                .into_any_element(),
        };
        let name_len = item.name.len();
        let ranges: Vec<std::ops::Range<usize>> = item
            .name
            .char_indices()
            .enumerate()
            .filter(|(i, _)| item.hits.contains(i))
            .map(|(_, (b, c))| b..b + c.len_utf8())
            .filter(|r| r.end <= name_len)
            .collect();
        let bold =
            HighlightStyle { color: Some(p.primary.into()), font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let name = StyledText::new(item.name.clone()).with_highlights(ranges.into_iter().map(|r| (r, bold)));
        let strong = active || item.unread > 0;
        let place = format!("{}|{}|{}", item.key, item.server, item.channel.as_deref().unwrap_or_default());
        let unread = (item.unread > 0).then(|| {
            motion::count(format!("switch-unread|{place}"), u64::from(item.unread), Some(99), 11.2, window, cx)
        });
        let pick = item.clone();
        let row = div()
            .id(SharedString::from(format!("switch-{n}")))
            .relative()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(radius_xl())
            .cursor_pointer()
            .text_color(if active { p.foreground } else { p.muted_foreground })
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if let Some(s) = &mut this.switcher
                    && s.active != n
                {
                    s.active = n;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| this.pick(pick.clone(), window, cx)))
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_base()
                            .line_height(px(24.0))
                            .when(strong, |el| el.font_weight(FontWeight::BOLD).text_color(p.foreground))
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(item.place.clone()),
                    ),
            )
            .when_some(unread, |el, unread| {
                el.child(
                    div()
                        .h(px(20.0))
                        .min_w(px(20.0))
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(p.destructive)
                        .text_color(gpui_kit::white())
                        .text_size(px(11.2))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(unread),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(p.muted_foreground)
                    .when(!active, |el| el.opacity(0.0))
                    .when(active, |el| {
                        el.child(motion::slide_in(
                            icon("corner-down-left").size(px(16.0)),
                            SharedString::from(format!("switch-go-{n}")),
                            -6.0,
                        ))
                    })
                    .when(!active, |el| el.child(icon("corner-down-left").size(px(16.0)))),
            );
        // Each place slides in as it turns up (the web's `x: -6`), one after another, 15ms apart.
        motion::slide_in_after(
            row,
            SharedString::from(format!("switch-in|{place}")),
            -6.0,
            Duration::from_millis(15 * n.min(10) as u64),
        )
        .into_any_element()
    }

    // ───────────────────────── The shortcut sheet ─────────────────────────

    /// Escape closes the sheet.
    fn sheet_keys(&mut self, key: &Keystroke, cx: &mut Context<Self>) -> bool {
        if !self.sheet_open || key.key != "escape" {
            return false;
        }
        self.sheet_open = false;
        cx.notify();
        true
    }

    pub(crate) fn render_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.sheet_open {
            return None;
        }
        let p = pal(cx);
        let prefs = &self.prefs;
        let mut row = 0usize;
        // `grid sm:grid-cols-2 gap-x-10 gap-y-6 px-8 py-4` across the sheet's 894px inside its border.
        let mut columns = div().flex().flex_wrap().gap_x(px(40.0)).gap_y(px(24.0)).px(px(32.0)).py(px(16.0));
        for group in Group::ALL {
            let mut section = div().w(px(395.0)).max_w_full().flex().flex_col().child(
                div()
                    .mb(px(4.0))
                    .text_size(px(11.2))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(group.name().to_uppercase(), WIDE)),
            );
            for action in keybinds::ACTIONS.iter().filter(|a| a.group == group) {
                let combo = keybinds::binding_of(action, &prefs.keybinds);
                let extra: Vec<&keybinds::CustomKeybind> =
                    prefs.custom_keybinds.iter().filter(|c| c.action == action.id).collect();
                let mut keys = div().flex().flex_wrap().justify_end().gap(px(6.0));
                match &combo {
                    Some(c) => keys = keys.child(keycaps(c, &p)),
                    None if extra.is_empty() => {
                        keys = keys.child(
                            div().text_xs().text_color(p.muted_foreground).child(t("chattools.shortcuts.notSet")),
                        )
                    }
                    None => {}
                }
                for c in extra {
                    keys = keys.child(
                        div().p(px(2.0)).rounded(corner(8.0)).bg(alpha(p.primary, 0.1)).child(keycaps(&c.combo, &p)),
                    );
                }
                section = section.child(sheet_row(action.label, keys, false, row, &p));
                row += 1;
            }
            if group == Group::Chat {
                for (label, combo) in COMPOSER_KEYS {
                    section = section.child(sheet_row(label, keycaps(combo, &p), true, row, &p));
                    row += 1;
                }
            }
            columns = columns.child(section);
        }
        let open_with = keybinds::action_by_id("shortcuts").and_then(|a| keybinds::binding_of(a, &prefs.keybinds));
        let footer = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(32.0))
            .py(px(12.0))
            .border_t_1()
            .border_color(p.border)
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .map(|el| match &open_with {
                        Some(c) => {
                            // One sentence with the keys drawn where {keys} sits in it.
                            let line = t_with("chattools.shortcuts.openAnytime", &[("keys", Arg::Str(KEYS_AT))]);
                            let (before, after) = line.split_once(KEYS_AT).unwrap_or((line.as_str(), ""));
                            el.when(!before.trim().is_empty(), |el| el.child(before.trim().to_owned()))
                                .child(keycaps(c, &p))
                                .when(!after.trim().is_empty(), |el| el.child(after.trim().to_owned()))
                        }
                        None => el.child(t("desktop.sheet.noShortcut")),
                    }),
            )
            .child(
                div()
                    .id("sheet-change")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .h(px(34.0))
                    .rounded(corner(12.0))
                    .bg(p.primary)
                    .text_color(p.primary_foreground)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .group("sheet-change")
                    // The web's `hover:brightness-110 active:scale-95`.
                    .hover({
                        let bright = mix(p.primary, gpui_kit::rgb(0xffffff), 0.1);
                        move |s| s.bg(bright)
                    })
                    .active(|s| s.scale(0.95))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.sheet_open = false;
                        this.open_settings(window, cx);
                        if let Some(s) = &this.settings {
                            s.update(cx, |s, cx| s.show_keyboard(cx));
                        }
                    }))
                    .child(
                        // The sparkles tilt as the button's pointed at.
                        div()
                            .id("sheet-change-sparkles")
                            .group_hover("sheet-change", |s| s.rotate(gpui_kit::radians(12f32.to_radians())))
                            .child(icon("sparkles").size(px(15.0))),
                    )
                    .child(t("chattools.shortcuts.change")),
            );
        let header = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(32.0))
            .pt(px(12.0))
            .pb(px(8.0))
            .child(motion::pop(
                div()
                    .size(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(12.0))
                    .bg(p.primary)
                    .text_color(p.primary_foreground)
                    .child(icon("keyboard").size(px(20.0))),
                "sheet-badge",
                0.6,
                -20.0,
                Duration::from_millis(80),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_lg()
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("chattools.shortcuts.title")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t("chattools.shortcuts.about")),
                    ),
            )
            .child(motion::answer(
                div()
                    .id("sheet-close")
                    .flex_shrink_0()
                    .size(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(p.muted_foreground)
                    .hover({
                        let (bg, fg) = (p.muted, p.foreground);
                        move |s| s.bg(bg).text_color(fg)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sheet_open = false;
                        cx.notify();
                    }))
                    .child(icon("x").size(px(18.0))),
                "sheet-close",
                motion::Pose::turn(90.0),
                motion::Pose::turn(90.0),
                window,
                cx,
            ));
        let sheet = div()
            .id("sheet")
            .occlude()
            .w_full()
            .max_w(px(896.0))
            .max_h(px(f32::from(window.viewport_size().height) * 0.85))
            .flex()
            .flex_col()
            .rounded_t(radius_3xl())
            .shadow(crate::ui::overlay::shadow_2xl())
            .border_1()
            .border_b_0()
            .border_color(p.border)
            .bg(p.card)
            .child(
                div().mx_auto().mt(px(10.0)).w(px(40.0)).h(px(6.0)).rounded_full().bg(alpha(p.muted_foreground, 0.3)),
            )
            .child(header)
            .child(div().id("sheet-body").flex_1().overflow_y_scroll().child(columns))
            .child(footer);
        Some(
            div()
                .id("sheet-scrim")
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .justify_end()
                .items_center()
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sheet_open = false;
                    cx.notify();
                }))
                // The web's `bg-black/40 backdrop-blur`, fading in.
                .child(motion::fade_in(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.4))
                        .backdrop_blur(px(crate::ui::overlay::SCRIM_BLUR)),
                    "sheet-scrim-in",
                    Duration::from_millis(200),
                ))
                .child(motion::sheet_up(sheet, "sheet-up"))
                .into_any_element(),
        )
    }
}

/// A row of the switcher: its 32px picture and two lines, and 8px above and below.
const SWITCH_ROW: f32 = 56.0;

/// Stands in for the keycaps while a sentence around them is translated.
const KEYS_AT: &str = "\u{E000}";

/// A line with its mark (`#`, `*`) in bold, wherever the language puts it.
fn marked(line: String, mark: &str, p: &Palette) -> impl IntoElement {
    let bold =
        HighlightStyle { color: Some(p.foreground.into()), font_weight: Some(FontWeight::BOLD), ..Default::default() };
    let ranges: Vec<_> = line.find(mark).map(|at| (at..at + mark.len(), bold)).into_iter().collect();
    div().child(StyledText::new(line).with_highlights(ranges))
}

fn sheet_row(label: &str, keys: impl IntoElement, muted: bool, n: usize, p: &Palette) -> impl IntoElement {
    motion::rise(
        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .min_h(px(40.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(alpha(p.border, 0.6))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .when(muted, |el| el.text_color(p.muted_foreground))
                    .when(!muted, |el| el.font_weight(FontWeight::BOLD))
                    .child(label.to_owned()),
            )
            .child(keys),
        SharedString::from(format!("sheet-row-{n}")),
        Duration::from_millis(60 + 18 * n.min(20) as u64),
        10.0,
    )
}

/// A combo as keycaps.
pub fn keycaps(combo: &str, p: &Palette) -> impl IntoElement {
    let mut row = div().flex().items_center().gap(px(4.0));
    for cap in keybinds::keycaps(combo) {
        row = row.child(
            div()
                .min_w(px(24.0))
                .h(px(24.0))
                .px(px(6.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(7.0))
                .border_1()
                .border_color(p.border)
                .border_b_2()
                .bg(p.secondary)
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .child(cap),
        );
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_letters_in_order_and_likes_starts() {
        assert!(fuzzy("gnrl", "general").is_some());
        assert!(fuzzy("xyz", "general").is_none());
        let (start, _) = fuzzy("gen", "general").unwrap();
        let (middle, _) = fuzzy("era", "general").unwrap();
        assert!(start > middle);
        assert_eq!(fuzzy("g e", "general").unwrap().1, vec![0, 1]);
        assert_eq!(fuzzy("", "anything").unwrap().1, Vec::<usize>::new());
    }

    #[test]
    fn key_presses_read_as_the_web_writes_them() {
        let k = |s: &str| Keystroke::parse(s).unwrap();
        let primary = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        assert_eq!(combo_of(&k(&format!("{primary}-k"))).as_deref(), Some("Mod+K"));
        assert_eq!(combo_of(&k("alt-shift-up")).as_deref(), Some("Alt+Shift+ArrowUp"));
        assert_eq!(combo_of(&k("shift-escape")).as_deref(), Some("Shift+Escape"));
        assert_eq!(combo_of(&k("tab")).as_deref(), Some("Tab"));
    }
}
