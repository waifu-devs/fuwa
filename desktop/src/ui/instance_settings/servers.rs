//! The Servers page: every community server on the instance, with its owner
//! and what it holds. Admins open one to change its caps, move it to another
//! region, end its shared channels, save its whole file or delete it,
//! whether or not they're in it. The web's `settings/instance/Servers.tsx`,
//! whose rows open in place; here a row opens the server's own view, so the
//! list can stay one row tall each and draw only the rows in sight.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, hsla,
    px, relative, uniform_list,
};

use super::controls::{count_text, number, size_text, switch_in};
use super::{InstanceSettingsEvent, InstanceSettingsView};
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin, format_bytes};
use crate::core::instance_manage as manage;
use crate::core::instance_servers::{self as servers, CAPS, Sort, members_of, storage_of};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::{amber, marked, pill, shimmer_rows, spinner, strong};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{card, danger_button, error_line, icon, primary_button, server_icon, soft_button};

/// A row's height, with the space under it; every row is as tall so the
/// list can skip what's out of sight.
const ROW: f32 = 74.0;
const GAP: f32 = 6.0;

/// What's open for one server; a fresh one each time a server opens.
#[derive(Default)]
struct Detail {
    /// Its own caps as saved (unset ones follow the instance's), and as edited.
    own: Option<pb::ServerLimits>,
    draft: pb::ServerLimits,
    saving: bool,
    save_error: Option<String>,
    shares: Option<Vec<pb::SharedConnection>>,
    /// The shared channel whose ending is being confirmed.
    ending: Option<pb::SharedConnection>,
    ending_busy: bool,
    ending_error: Option<String>,
    /// The region it's being moved to, while that's being confirmed.
    move_to: Option<String>,
    moving: bool,
    move_error: Option<String>,
    deleting: bool,
    delete_busy: bool,
    delete_error: Option<String>,
}

/// A server's file being saved: which server, and how far along (an f32's bits).
struct Export {
    server: String,
    progress: Arc<AtomicU32>,
}

pub(super) struct Servers {
    query: Entity<InputState>,
    sort: Sort,
    list: Option<Vec<pb::InstanceServer>>,
    error: Option<String>,
    asked: bool,
    /// Pictures people keep here: how many, and their bytes.
    pictures: (i64, i64),
    defaults: pb::ServerLimits,
    scroll: gpui_kit::UniformListScrollHandle,
    /// The server open now.
    open: Option<String>,
    /// Bumped each time a server opens, so a late answer about another is dropped.
    opened: u64,
    detail: Detail,
    /// The boxes the six caps are typed in, and the unit each size is in.
    caps: Vec<Entity<InputState>>,
    units: [usize; 6],
    confirm: Entity<InputState>,
    export: Option<Export>,
    exported: Option<Instant>,
}

impl Servers {
    pub fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("instancesettings.servers.search")));
        let confirm = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![
            cx.subscribe(&query, |this: &mut InstanceSettingsView, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.servers.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
                    cx.notify();
                }
            }),
            cx.subscribe(&confirm, |_: &mut InstanceSettingsView, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut caps = Vec::new();
        for (n, (_, bytes, read, write)) in CAPS.into_iter().enumerate() {
            let state = cx.new(|cx| InputState::new(window, cx));
            subs.push(cx.subscribe(&state, move |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                cx.notify();
                let unit = bytes.then(|| this.servers.units[n]);
                let Some(value) = admin::parse_cap(&state.read(cx).value(), unit) else { return };
                // A cap that's off stays off while its box is filled in.
                let d = &mut this.servers.detail;
                if read(&d.draft).is_some_and(|now| now != value) {
                    write(&mut d.draft, Some(value));
                    d.save_error = None;
                }
            }));
            caps.push(state);
        }
        (
            Self {
                query,
                sort: Sort::Biggest,
                list: None,
                error: None,
                asked: false,
                pictures: (0, 0),
                defaults: pb::ServerLimits::default(),
                scroll: gpui_kit::UniformListScrollHandle::new(),
                open: None,
                opened: 0,
                detail: Detail::default(),
                caps,
                units: [1; 6],
                confirm,
                export: None,
                exported: None,
            },
            subs,
        )
    }
}

fn id_of(s: &pb::InstanceServer) -> String {
    s.server.as_ref().map(|s| s.id.clone()).unwrap_or_default()
}

fn name_of(s: &pb::InstanceServer) -> String {
    s.server.as_ref().map(|s| s.name.clone()).unwrap_or_default()
}

fn group(n: f64) -> String {
    number(n.round() as i64)
}

/// A region's name, or "Home" for an unnamed home region.
fn region_name(regions: &[pb::Region], region: &str) -> String {
    servers::region_name(regions, region).unwrap_or_else(|| t("desktop.instance.homeRegion"))
}

/// One of a server's caps, by its id in `CAPS`.
fn cap_label(id: &str) -> String {
    match id {
        "members" => t("serversettings.nav.members"),
        "channels" => t("serversettings.nav.channels"),
        "storage" => t("serversettings.usage.storage"),
        "files" => t("serversettings.limits.files"),
        "emoji" => t("serversettings.nav.emoji"),
        _ => t("serversettings.nav.recordings"),
    }
}

/// Who owns a server and when it was made, as a byline.
fn byline(owner: Option<&pb::User>, made: Option<&str>) -> String {
    match (owner, made) {
        (Some(o), Some(day)) => t_with(
            "instancesettings.servers.byline",
            &[("name", Arg::Str(&user_name(o))), ("username", Arg::Str(&o.username)), ("day", Arg::Str(day))],
        ),
        (Some(o), None) => t_with(
            "desktop.instance.byOwner",
            &[("name", Arg::Str(&user_name(o))), ("username", Arg::Str(&o.username))],
        ),
        (None, Some(day)) => t_with("instancesettings.servers.bylineNobody", &[("day", Arg::Str(day))]),
        (None, None) => t("desktop.instance.byNobody"),
    }
}

fn bytes(n: f64) -> String {
    format_bytes(n.round() as i64)
}

fn green() -> Hsla {
    hsla(0.42, 0.65, 0.45, 1.0)
}

/// A small heading over a part of the page.
fn heading(glyph: Option<&str>, text: &str, p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .when_some(glyph, |el, g| el.child(icon(g).size(px(14.0))))
        .child(text.to_uppercase())
}

/// A quiet button with an icon before its words.
fn icon_soft(
    id: impl Into<SharedString>,
    glyph: &str,
    label: impl Into<SharedString>,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    soft_button(id.into(), "", p).child(icon(glyph).size(px(15.0))).child(label.into())
}

/// A count or size in a tile, rolling up when it shows.
fn tile(id: String, glyph: Option<&str>, label: &str, value: f64, sized: bool, n: usize, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex_1()
            .min_w_0()
            .p(px(12.0))
            .rounded(corner(16.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .child(heading(glyph, label, p))
            .child(div().mt(px(4.0)).text_xl().font_weight(FontWeight::EXTRA_BOLD).text_ellipsis().child(
                motion::count_up(
                    SharedString::from(format!("{id}-count")),
                    value,
                    Duration::from_millis(100 + 50 * n as u64),
                    if sized { bytes } else { group },
                ),
            )),
        SharedString::from(id),
        Duration::from_millis(50 * n as u64),
        10.0,
    )
    .into_any_element()
}

impl InstanceSettingsView {
    fn regions(&self) -> Vec<pb::Region> {
        self.core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.node.as_ref().map(|n| n.regions.clone())))
            .unwrap_or_default()
    }

    fn load_servers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.servers.asked = true;
        self.servers.error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.list_instance_servers(&key).await }, |this, result, _, cx| {
            match result {
                Ok(list) => this.servers.list = Some(list),
                Err(problem) => this.servers.error = Some(problem.message),
            }
            cx.notify();
        });
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.node_usage(&key).await }, |this, result, _, cx| {
            if let Ok(usage) = result {
                this.servers.pictures = (usage.pictures, usage.picture_bytes);
                this.servers.defaults = usage.default_limits.unwrap_or_default();
            }
            cx.notify();
        });
    }

    fn entry(&self, id: &str) -> Option<&pb::InstanceServer> {
        self.servers.list.as_ref()?.iter().find(|s| id_of(s) == id)
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut pb::InstanceServer> {
        self.servers.list.as_mut()?.iter_mut().find(|s| id_of(s) == id)
    }

    /// Opens one server: its caps and shared channels load as it slides in.
    fn open_server(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.servers.opened += 1;
        let n = self.servers.opened;
        self.servers.open = Some(id.clone());
        self.servers.detail = Detail::default();
        let confirm = self.servers.confirm.clone();
        confirm.update(cx, |s, cx| s.set_value("", window, cx));
        let (core, key, server) = (self.core.clone(), self.key.clone(), id.clone());
        self.run(
            window,
            cx,
            async move { core.server_own_limits(&key, &server).await },
            move |this, result, window, cx| {
                if this.servers.opened != n {
                    return;
                }
                match result {
                    Ok(own) => {
                        this.servers.detail.draft = own;
                        this.servers.detail.own = Some(own);
                        this.sync_server_caps(window, cx);
                    }
                    Err(problem) => this.toast("circle-alert", problem.message, cx),
                }
                cx.notify();
            },
        );
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.list_server_shares(&key, &id).await }, move |this, result, _, cx| {
            if this.servers.opened != n {
                return;
            }
            match result {
                Ok(shares) => this.servers.detail.shares = Some(shares),
                // An instance from before shared channels has none to show.
                Err(problem) if problem.code == tonic::Code::Unimplemented => {
                    this.servers.detail.shares = Some(Vec::new())
                }
                Err(problem) => this.toast("circle-alert", problem.message, cx),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Back to the list.
    fn close_server(&mut self, cx: &mut Context<Self>) {
        self.servers.opened += 1;
        self.servers.open = None;
        self.servers.detail = Detail::default();
        cx.notify();
    }

    /// The menu's Servers again: back to the list.
    pub(super) fn servers_back(&mut self, cx: &mut Context<Self>) {
        if self.servers.open.is_some() && self.servers.detail.ending.is_none() {
            self.close_server(cx);
        }
    }

    /// Esc on this page: a dialog first, then the open server. False when neither.
    pub(super) fn escape_servers(&mut self, cx: &mut Context<Self>) -> bool {
        if self.servers.detail.ending.is_some() {
            if !self.servers.detail.ending_busy {
                self.servers.detail.ending = None;
                cx.notify();
            }
            return true;
        }
        if self.servers.open.is_some() {
            self.close_server(cx);
            return true;
        }
        false
    }

    /// Puts the draft's caps in their boxes where they don't already say it.
    fn sync_server_caps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.servers.detail.draft;
        for (n, (_, sized, read, _)) in CAPS.into_iter().enumerate() {
            let state = self.servers.caps[n].clone();
            let value = read(&draft);
            let unit = sized.then(|| self.servers.units[n]);
            if value.is_some() && admin::parse_cap(&state.read(cx).value(), unit) == value {
                continue;
            }
            let (text, unit) =
                if sized { admin::split_bytes(value) } else { (value.map(|v| v.to_string()).unwrap_or_default(), 1) };
            self.servers.units[n] = unit;
            state.update(cx, |s, cx| s.set_value(text, window, cx));
        }
    }

    /// Turns one of the server's caps on (with what's typed, or 100, or 1 GB) or back to the default.
    fn switch_server_cap(&mut self, n: usize, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (_, sized, _, write) = CAPS[n];
        self.servers.detail.save_error = None;
        if !on {
            write(&mut self.servers.detail.draft, None);
            cx.notify();
            return;
        }
        let state = self.servers.caps[n].clone();
        let unit = sized.then(|| self.servers.units[n]);
        let value = match admin::parse_cap(&state.read(cx).value(), unit) {
            Some(v) => v,
            None => {
                let (text, v) = if sized { ("1", admin::UNITS[1].1) } else { ("100", 100) };
                self.servers.units[n] = 1;
                state.update(cx, |s, cx| s.set_value(text, window, cx));
                v
            }
        };
        write(&mut self.servers.detail.draft, Some(value));
        state.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn pick_server_unit(&mut self, n: usize, unit: usize, cx: &mut Context<Self>) {
        self.servers.units[n] = unit;
        let typed = self.servers.caps[n].read(cx).value().to_string();
        if let Some(value) = admin::parse_cap(&typed, Some(unit)) {
            (CAPS[n].3)(&mut self.servers.detail.draft, Some(value));
        }
        cx.notify();
    }

    fn caps_changed(&self) -> usize {
        let d = &self.servers.detail;
        let Some(own) = &d.own else { return 0 };
        CAPS.iter().filter(|(_, _, read, _)| read(&d.draft) != read(own)).count()
    }

    fn discard_server_caps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(own) = self.servers.detail.own else { return };
        self.servers.detail.draft = own;
        self.servers.detail.save_error = None;
        self.sync_server_caps(window, cx);
        cx.notify();
    }

    fn save_server_caps(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.servers.open.clone() else { return };
        let d = &mut self.servers.detail;
        if d.saving {
            return;
        }
        d.saving = true;
        d.save_error = None;
        let draft = d.draft;
        let name = self.entry(&id).map(name_of).unwrap_or_default();
        let n = self.servers.opened;
        let (core, key, server) = (self.core.clone(), self.key.clone(), id.clone());
        self.run(
            window,
            cx,
            async move { core.set_server_limits(&key, &server, draft).await.map(|l| (l, draft)) },
            move |this, result, _, cx| {
                if let Ok((limits, _)) = &result
                    && let Some(entry) = this.entry_mut(&id)
                {
                    entry.limits = Some(*limits);
                }
                if this.servers.opened == n {
                    let d = &mut this.servers.detail;
                    d.saving = false;
                    match result {
                        Ok((_, saved)) => {
                            d.own = Some(saved);
                            this.toast(
                                "check",
                                t_with("instancesettings.servers.capsSaved", &[("server", Arg::Str(&name))]),
                                cx,
                            );
                        }
                        Err(problem) => d.save_error = Some(problem.message),
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn move_open_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some(to)) = (self.servers.open.clone(), self.servers.detail.move_to.clone()) else { return };
        if self.servers.detail.moving {
            return;
        }
        self.servers.detail.moving = true;
        self.servers.detail.move_error = None;
        let regions = self.regions();
        let place = region_name(&regions, &to);
        let n = self.servers.opened;
        let (core, key, server) = (self.core.clone(), self.key.clone(), id.clone());
        self.run(window, cx, async move { core.move_server(&key, &server, &to).await }, move |this, result, _, cx| {
            if let Ok(moved) = &result
                && let Some(entry) = this.entry_mut(&id)
            {
                entry.server = Some(moved.clone());
            }
            if this.servers.opened == n {
                let d = &mut this.servers.detail;
                d.moving = false;
                match result {
                    Ok(moved) => {
                        d.move_to = None;
                        this.toast(
                            "plane",
                            t_with(
                                "instancesettings.servers.movedTo",
                                &[("server", Arg::Str(&moved.name)), ("region", Arg::Str(&place))],
                            ),
                            cx,
                        );
                    }
                    Err(problem) => d.move_error = Some(problem.message),
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    fn end_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some(ending)) = (self.servers.open.clone(), self.servers.detail.ending.clone()) else { return };
        if self.servers.detail.ending_busy {
            return;
        }
        self.servers.detail.ending_busy = true;
        self.servers.detail.ending_error = None;
        let n = self.servers.opened;
        let (core, key, connection) = (self.core.clone(), self.key.clone(), ending.id.clone());
        self.run(
            window,
            cx,
            async move { core.end_server_share(&key, &id, &connection).await },
            move |this, result, _, cx| {
                if this.servers.opened != n {
                    return;
                }
                let d = &mut this.servers.detail;
                d.ending_busy = false;
                match result {
                    Ok(()) => {
                        if let Some(list) = &mut d.shares {
                            list.retain(|c| c.id != ending.id);
                        }
                        d.ending = None;
                        let ended = t_with(
                            "instancesettings.servers.ended",
                            &[("channel", Arg::Str(&channel_of(&ending))), ("server", Arg::Str(&other_of(&ending)))],
                        );
                        this.toast("unlink", ended, cx);
                    }
                    Err(problem) => d.ending_error = Some(problem.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Asks where to save the open server's file, then fills it as it arrives.
    fn export_open_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.servers.open.clone() else { return };
        if self.servers.export.is_some() {
            return;
        }
        let name = servers::export_name(&self.entry(&id).map(name_of).unwrap_or_default(), &servers::today());
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        let (core, key) = (self.core.clone(), self.key.clone());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let progress = Arc::new(AtomicU32::new(0));
            let started = this.update(cx, |this, cx| {
                if this.servers.export.is_some() {
                    return false;
                }
                this.servers.export = Some(Export { server: id.clone(), progress: progress.clone() });
                this.servers.exported = None;
                cx.notify();
                true
            });
            if !matches!(started, Ok(true)) {
                return;
            }
            let rx = core.spawn({
                let (core, progress) = (core.clone(), progress.clone());
                async move {
                    core.export_server(&key, &id, &path, move |f| progress.store(f.to_bits(), Ordering::Relaxed)).await
                }
            });
            // The bar fills as pieces arrive.
            let ticker = this.clone();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(120)).await;
                    let going = ticker.update(cx, |this, cx| {
                        cx.notify();
                        this.servers.export.is_some()
                    });
                    if !matches!(going, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.servers.export = None;
                match result {
                    Ok(Ok(size)) => {
                        this.servers.exported = Some(Instant::now());
                        let saved = t_with(
                            "instancesettings.servers.savedFile",
                            &[("file", Arg::Str(&name)), ("size", Arg::Str(&format_bytes(size as i64)))],
                        );
                        this.toast("download", saved, cx);
                    }
                    Ok(Err(problem)) => this.toast("circle-alert", problem.message, cx),
                    Err(_) => {}
                }
                cx.notify();
            });
            cx.background_executor().timer(Duration::from_millis(2300)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn delete_open_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.servers.open.clone() else { return };
        let name = self.entry(&id).map(name_of).unwrap_or_default();
        let d = &mut self.servers.detail;
        if d.delete_busy || self.servers.confirm.read(cx).value().as_ref() != name {
            return;
        }
        d.delete_busy = true;
        d.delete_error = None;
        let n = self.servers.opened;
        let (core, key, server) = (self.core.clone(), self.key.clone(), id.clone());
        self.run(window, cx, async move { core.delete_any_server(&key, &server).await }, move |this, result, _, cx| {
            match result {
                Ok(()) => {
                    if let Some(list) = &mut this.servers.list {
                        list.retain(|s| id_of(s) != id);
                    }
                    if this.servers.opened == n {
                        this.close_server(cx);
                    }
                    this.toast("trash", t_with("serversettings.shared.deleted", &[("name", Arg::Str(&name))]), cx);
                }
                Err(problem) if this.servers.opened == n => {
                    this.servers.detail.delete_busy = false;
                    this.servers.detail.delete_error = Some(problem.message);
                }
                Err(_) => {}
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn servers_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.servers.asked {
            self.load_servers(window, cx);
        }
        if let Some(error) = &self.servers.error {
            return div().text_sm().text_color(p.muted_foreground).child(error.clone()).into_any_element();
        }
        let Some(list) = self.servers.list.as_ref() else {
            return div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(div().flex().gap(px(8.0)).children(
                    (0..4).map(|_| div().flex_1().h(px(80.0)).rounded(corner(16.0)).bg(alpha(p.muted_foreground, 0.1))),
                ))
                .child(shimmer_rows(3, p))
                .into_any_element();
        };
        if let Some(id) = self.servers.open.clone()
            && let Some(entry) = list.iter().find(|s| id_of(s) == id).cloned()
        {
            return self.server_detail(&entry, p, window, cx);
        }
        self.server_list(p, window, cx)
    }

    /// Whether the page is the list, which scrolls by itself.
    pub(super) fn servers_fill(&self) -> bool {
        self.servers.open.is_none() && self.servers.list.is_some()
    }

    fn server_list(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let list = self.servers.list.as_deref().unwrap_or_default();
        let memberships: i64 = list.iter().map(members_of).sum();
        let storage: i64 = list.iter().map(storage_of).sum();
        let (pictures, picture_bytes) = self.servers.pictures;
        let totals = div()
            .flex()
            .gap(px(8.0))
            .child(tile(
                "servers-total-servers".into(),
                Some("server"),
                &t("instancesettings.nav.servers"),
                list.len() as f64,
                false,
                0,
                p,
            ))
            .child(tile(
                "servers-total-members".into(),
                Some("users"),
                &t("instancesettings.servers.memberships"),
                memberships as f64,
                false,
                1,
                p,
            ))
            .child(tile(
                "servers-total-storage".into(),
                Some("hard-drive"),
                &t("serversettings.usage.storage"),
                storage as f64,
                true,
                2,
                p,
            ))
            .child(
                div()
                    .id("servers-total-pictures")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(t_with(
                            "instancesettings.servers.picturesSub",
                            &[("count", Arg::Num(pictures))],
                        ))
                        .build(window, cx)
                    })
                    .child(tile(
                        format!("servers-total-pictures-{picture_bytes}"),
                        Some("image"),
                        &t("instancesettings.servers.pictures"),
                        picture_bytes as f64,
                        true,
                        3,
                        p,
                    )),
            );

        let query = self.servers.query.read(cx).value().to_string();
        let sort = self.servers.sort;
        let shown: Vec<String> = servers::shown(list, &query, sort).into_iter().map(id_of).collect();
        let biggest = list.iter().map(storage_of).max().unwrap_or(0).max(1);
        let regions = self.regions();
        let top = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(div().flex_1().min_w_0().child(
                Input::new(&self.servers.query).prefix(icon("search").size(px(15.0)).text_color(p.muted_foreground)),
            ))
            .child(sorter(sort, p, window, cx));
        let count = shown.len();
        let header =
            heading(Some("server"), &t_with("instancesettings.servers.count", &[("count", Arg::Num(count as i64))]), p);
        let column = div().flex_1().min_h_0().flex().flex_col().gap(px(10.0)).child(header);
        let body = if count == 0 {
            column.child(motion::rise(
                div().py(px(32.0)).flex().justify_center().text_sm().text_color(p.muted_foreground).child(
                    if query.trim().is_empty() {
                        t("instancesettings.servers.none")
                    } else {
                        t("instancesettings.servers.noMatch")
                    },
                ),
                SharedString::from(format!("servers-none-{}", query.trim().is_empty())),
                Duration::ZERO,
                8.0,
            ))
        } else {
            let now = now_ms();
            let rows = uniform_list(
                "server-rows",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let p = crate::ui::widgets::pal(cx);
                    range
                        .map(|n| {
                            let entry = shown.get(n).and_then(|id| this.entry(id)).cloned();
                            let el = match entry {
                                Some(entry) => this.server_row(&entry, n, biggest, &regions, now, &p, cx),
                                None => div().into_any_element(),
                            };
                            div().h(px(ROW)).pb(px(GAP)).child(el).into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.servers.scroll)
            .flex_1()
            .min_h_0();
            column.child(rows)
        };
        div().flex_1().min_h_0().flex().flex_col().gap(px(16.0)).child(totals).child(top).child(body).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn server_row(
        &self,
        entry: &pb::InstanceServer,
        index: usize,
        biggest: i64,
        regions: &[pb::Region],
        now: i64,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = entry.server.as_ref() else { return div().into_any_element() };
        let id = s.id.clone();
        let storage = storage_of(entry);
        let cap = entry.limits.as_ref().and_then(|l| l.storage_bytes);
        let share = match cap {
            Some(cap) if cap > 0 => (storage as f32 / cap as f32).min(1.0),
            _ => storage as f32 / biggest as f32,
        };
        let full = cap.is_some() && share >= 0.9;
        let made =
            s.created_at.as_ref().map(|t| manage::day_label(t.seconds * 1000, now).to_lowercase()).unwrap_or_default();
        let by = byline(entry.owner.as_ref(), Some(&made));
        let bar_color: Hsla = if full { amber(p) } else { alpha(p.primary, 0.7) };
        let line = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .child(
                div().min_w_0().text_ellipsis().whitespace_nowrap().font_weight(FontWeight::BOLD).child(s.name.clone()),
            )
            .child(icon(if s.discoverable { "globe" } else { "eye-off" }).size(px(13.0)).text_color(p.muted_foreground))
            .when(servers::has_regions(regions), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(2.0))
                        .px(px(6.0))
                        .h(px(18.0))
                        .rounded_full()
                        .bg(p.muted)
                        .text_color(p.muted_foreground)
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .child(icon("map-pin").size(px(10.0)))
                        .child(region_name(regions, &s.region)),
                )
            })
            .when(entry.member, |el| {
                el.child(pill(&t("instancesettings.servers.youreIn").to_uppercase(), p.primary.into()))
            });
        let numbers = div()
            .flex()
            .flex_none()
            .flex_col()
            .items_end()
            .text_xs()
            .text_color(p.muted_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("users").size(px(12.0)))
                    .child(number(members_of(entry))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .when(full, |el| el.text_color(amber(p)).font_weight(FontWeight::BOLD))
                    .child(icon("hard-drive").size(px(12.0)))
                    .child(format_bytes(storage))
                    .when_some(cap, |el, cap| {
                        el.child(
                            div().text_color(alpha(p.muted_foreground, 0.7)).child(format!("/ {}", format_bytes(cap))),
                        )
                    }),
            );
        let delay = Duration::from_millis(100 + 30 * index.min(14) as u64);
        let bar = div()
            .absolute()
            .left(px(14.0))
            .right(px(14.0))
            .bottom_0()
            .h(px(2.0))
            .rounded_full()
            .overflow_hidden()
            .bg(alpha(p.muted, 0.6))
            .child(motion::once(
                div().h_full().rounded_full().bg(bar_color),
                SharedString::from(format!("server-bar-{id}-{}", (share * 1000.0) as u32)),
                delay + Duration::from_millis(900),
                move |el, t| {
                    let start = delay.as_secs_f32() / (delay.as_secs_f32() + 0.9);
                    let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                    let eased = 1.0 - (1.0 - k).powi(4);
                    el.w(relative(share.max(0.02) * eased))
                },
            ));
        let hover = alpha(p.primary, 0.3);
        let open = id.clone();
        let row =
            div()
                .id(SharedString::from(format!("server-{id}")))
                .relative()
                .h_full()
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(12.0))
                .rounded(corner(16.0))
                .bg(alpha(p.background, 0.4))
                .border_1()
                .border_color(p.border)
                .overflow_hidden()
                .cursor_pointer()
                .hover(move |s| s.border_color(hover))
                .active(|s| s.top(px(1.0)))
                .on_click(cx.listener(move |this, _, window, cx| this.open_server(open.clone(), window, cx)))
                .child(server_icon(s, 40.0, 12.0, p))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.0)).child(line).child(
                    div().text_xs().text_color(p.muted_foreground).text_ellipsis().whitespace_nowrap().child(by),
                ))
                .child(numbers)
                .child(icon("chevron-right").size(px(16.0)).text_color(p.muted_foreground))
                .child(bar);
        motion::rise(
            row,
            SharedString::from(format!("server-in-{id}")),
            Duration::from_millis(20 * index.min(14) as u64),
            10.0,
        )
        .into_any_element()
    }

    /// One server, opened: what it holds and what an admin can do with it.
    fn server_detail(
        &mut self,
        entry: &pb::InstanceServer,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = entry.server.clone().unwrap_or_default();
        let id = s.id.clone();
        let now = now_ms();
        let regions = self.regions();
        let back = div()
            .id("servers-back")
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover({
                let c = p.primary;
                move |s| s.text_color(c)
            })
            .on_click(cx.listener(|this, _, _, cx| this.close_server(cx)))
            .child(icon("arrow-left").size(px(16.0)))
            .child(t("desktop.instance.allServers"));
        let made = s.created_at.as_ref().map(|t| manage::day_label(t.seconds * 1000, now).to_lowercase());
        let by = byline(entry.owner.as_ref(), made.as_deref());
        let head = div().flex().items_center().gap(px(14.0)).child(server_icon(&s, 56.0, 16.0, p)).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div().text_xl().font_weight(FontWeight::EXTRA_BOLD).text_ellipsis().child(s.name.clone()),
                        )
                        .child(pill(
                            &if s.discoverable {
                                t("desktop.instance.inBrowse")
                            } else {
                                t("desktop.instance.hiddenFromBrowse")
                            }
                            .to_uppercase(),
                            p.muted_foreground.into(),
                        ))
                        .when(entry.member, |el| {
                            el.child(pill(&t("instancesettings.servers.youreIn").to_uppercase(), p.primary.into()))
                        }),
                )
                .child(div().text_sm().text_color(p.muted_foreground).text_ellipsis().child(by)),
        );
        let u = entry.usage.clone().unwrap_or_default();
        let stats = div()
            .flex()
            .gap(px(8.0))
            .child(tile(
                format!("server-{id}-members"),
                None,
                &t("serversettings.nav.members"),
                u.members as f64,
                false,
                0,
                p,
            ))
            .child(tile(
                format!("server-{id}-channels"),
                None,
                &t("serversettings.nav.channels"),
                u.channels as f64,
                false,
                1,
                p,
            ))
            .child(tile(
                format!("server-{id}-messages"),
                None,
                &t("serversettings.usage.messages"),
                u.messages as f64,
                false,
                2,
                p,
            ))
            .child(tile(
                format!("server-{id}-files"),
                None,
                &t("serversettings.limits.files"),
                u.attachment_bytes as f64,
                true,
                3,
                p,
            ));

        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(back)
            .child(head)
            .when(!s.description.is_empty(), |el| {
                el.child(div().text_sm().text_color(p.muted_foreground).child(s.description.clone()))
            })
            .child(stats)
            .child(self.server_caps(&id, p, window, cx));
        if servers::has_regions(&regions) {
            page = page.child(self.region_picker(&s, &regions, p, window, cx));
        }
        if let Some(shares) = self.shares_list(&id, p, cx) {
            page = page.child(shares);
        }
        page = page.child(self.server_actions(entry, p, window, cx));
        motion::slide_in(page, SharedString::from(format!("server-open-{id}")), 24.0).into_any_element()
    }

    fn server_caps(&mut self, id: &str, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut section =
            div().flex().flex_col().gap(px(12.0)).child(heading(None, &t("instancesettings.servers.caps"), p));
        let d = &self.servers.detail;
        if d.own.is_none() {
            return section
                .child(div().h(px(160.0)).rounded(corner(12.0)).bg(alpha(p.muted_foreground, 0.1)))
                .into_any_element();
        }
        for (n, (cap, sized, read, _)) in CAPS.into_iter().enumerate() {
            let value = read(&d.draft);
            let fallback = read(&self.servers.defaults);
            let default = t_with(
                "instancesettings.servers.defaultIs",
                &[("value", Arg::Str(&if sized { size_text(fallback) } else { count_text(fallback) }))],
            );
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .min_h(px(36.0))
                .child(switch_in(
                    SharedString::from(format!("scap-{id}-{n}")),
                    value.is_some(),
                    false,
                    cx,
                    move |this, on, window, cx| this.switch_server_cap(n, on, window, cx),
                ))
                .child(div().w(px(96.0)).flex_none().text_sm().font_weight(FontWeight::BOLD).child(cap_label(cap)));
            if value.is_none() {
                row = row.child(motion::slide_in(
                    div().text_sm().text_color(p.muted_foreground).child(default),
                    SharedString::from(format!("scap-{id}-{n}-off")),
                    12.0,
                ));
            } else {
                let mut amount = div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().w(px(112.0)).child(Input::new(&self.servers.caps[n])));
                if sized {
                    let unit = self.servers.units[n];
                    let x =
                        motion::follow(SharedString::from(format!("scap-{n}-unit")), unit as f32 * 40.0, window, cx);
                    let mut units = div().relative().flex().p(px(2.0)).rounded(corner(9.0)).bg(p.muted).child(
                        div()
                            .absolute()
                            .top(px(2.0))
                            .left(px(2.0 + x))
                            .w(px(40.0))
                            .h(px(26.0))
                            .rounded(corner(7.0))
                            .bg(p.background),
                    );
                    for (k, (name, _)) in admin::UNITS.iter().enumerate() {
                        units = units.child(
                            div()
                                .id(SharedString::from(format!("scap-{n}-{name}")))
                                .relative()
                                .w(px(40.0))
                                .h(px(26.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(if k == unit { p.foreground } else { p.muted_foreground })
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| this.pick_server_unit(n, k, cx)))
                                .child(*name),
                        );
                    }
                    amount = amount.child(units);
                }
                row = row.child(motion::slide_in(amount, SharedString::from(format!("scap-{id}-{n}-on")), -12.0));
            }
            section = section.child(row);
        }
        let changed = self.caps_changed();
        let d = &self.servers.detail;
        let saving = d.saving;
        section = section.when_some(error_line(d.save_error.as_deref(), p), |el, e| el.child(e));
        if changed > 0 {
            section = section.child(motion::rise(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        soft_button("scap-discard", t("settings.controls.discard"), p)
                            .on_click(cx.listener(|this, _, window, cx| this.discard_server_caps(window, cx))),
                    )
                    .child(
                        primary_button(
                            "scap-save",
                            if saving { t("settings.controls.saving") } else { t("instancesettings.servers.saveCaps") },
                            p,
                        )
                        .when(saving, |el| el.opacity(0.7))
                        .on_click(cx.listener(|this, _, window, cx| this.save_server_caps(window, cx))),
                    ),
                SharedString::from(format!("scap-bar-{id}")),
                Duration::ZERO,
                8.0,
            ));
        }
        section.into_any_element()
    }

    fn region_picker(
        &mut self,
        s: &pb::Server,
        regions: &[pb::Region],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let d = &self.servers.detail;
        let moving = d.moving;
        let mut chips = div().flex().flex_wrap().gap(px(8.0));
        for r in regions {
            let current = servers::same_region(regions, &r.id, &s.region);
            let on = if current { d.move_to.is_none() } else { d.move_to.as_deref() == Some(r.id.as_str()) };
            let rid = r.id.clone();
            let edge: Hsla = if on { p.primary.into() } else { p.border.into() };
            let hover = alpha(p.primary, 0.5);
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("region-{}-{}", s.id, r.id)))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(5.0))
                    .pl(px(5.0))
                    .pr(px(12.0))
                    .rounded_full()
                    .border_1()
                    .border_color(edge)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .when(on, |el| el.text_color(p.primary))
                    .when(!on, |el| el.hover(move |s| s.border_color(hover)))
                    .when(moving, |el| el.opacity(0.6))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.servers.detail.moving {
                            return;
                        }
                        this.servers.detail.move_to = if current { None } else { Some(rid.clone()) };
                        this.servers.detail.move_error = None;
                        cx.notify();
                    }))
                    .when(on, |el| {
                        el.child(motion::once(
                            div().absolute().inset_0().rounded_full().bg(alpha(p.primary, 0.15)),
                            SharedString::from(format!("region-on-{}-{}", s.id, r.id)),
                            Duration::from_millis(260),
                            |el, t| el.opacity(t),
                        ))
                    })
                    .child(
                        div()
                            .relative()
                            .size(px(24.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .bg(if on { p.primary } else { p.muted })
                            .text_color(if on { p.primary_foreground } else { p.muted_foreground })
                            .child(servers::region_mark(&r.name)),
                    )
                    .child(div().relative().child(r.name.clone()))
                    .when(current, |el| {
                        el.child(
                            div()
                                .relative()
                                .text_xs()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(p.muted_foreground)
                                .child(t("instancesettings.servers.now")),
                        )
                    }),
            );
        }
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(heading(Some("map-pin"), &t("instancesettings.servers.region"), p))
            .child(chips);
        let target = d.move_to.as_ref().and_then(|to| regions.iter().find(|r| &r.id == to));
        if let Some(target) = target {
            let here = region_name(regions, &s.region);
            let to = target.name.clone();
            let plane = icon("plane").size(px(16.0)).text_color(p.primary);
            // The plane waits halfway, and flies across while the server travels.
            let lane = div()
                .relative()
                .flex_1()
                .min_w(px(48.0))
                .h(px(16.0))
                .child(div().absolute().left_0().right_0().top(px(8.0)).h(px(1.0)).bg(p.border));
            let lane = if moving {
                lane.child(motion::ambient(
                    div().absolute().top_0().child(plane),
                    SharedString::from(format!("region-plane-fly-{}", s.id)),
                    Duration::from_millis(1100),
                    window,
                    |el, t| el.left(relative(0.88 * t)).opacity((t * 4.0).min(1.0).min((1.0 - t) * 4.0)),
                ))
            } else {
                let x = motion::follow(SharedString::from(format!("region-plane-{}", s.id)), 0.44, window, cx);
                lane.child(div().absolute().top_0().left(relative(x)).child(plane))
            };
            let d = &self.servers.detail;
            section = section.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(corner(16.0))
                    .border_1()
                    .border_color(alpha(p.primary, 0.3))
                    .bg(alpha(p.primary, 0.05))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(div().text_ellipsis().child(here.clone()))
                            .child(lane)
                            .child(div().text_ellipsis().text_color(p.primary).child(to.clone())),
                    )
                    .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                        "instancesettings.servers.moveWhat",
                        &[("to", Arg::Str(&to)), ("from", Arg::Str(&here))],
                    )))
                    .child(div().text_sm().text_color(p.muted_foreground).child(t("instancesettings.servers.moveWait")))
                    .when_some(error_line(d.move_error.as_deref(), p), |el, e| el.child(e))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                soft_button("region-cancel", t("common.cancel"), p)
                                    .when(moving, |el| el.opacity(0.6))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.servers.detail.moving {
                                            this.servers.detail.move_to = None;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                primary_button(
                                    "region-go",
                                    if moving {
                                        t_with("instancesettings.servers.moving", &[("region", Arg::Str(&to))])
                                    } else {
                                        t_with("instancesettings.servers.moveTo", &[("region", Arg::Str(&to))])
                                    },
                                    p,
                                )
                                .when(moving, |el| el.opacity(0.7))
                                .on_click(cx.listener(|this, _, window, cx| this.move_open_server(window, cx))),
                            ),
                    ),
                SharedString::from(format!("region-confirm-{}-{}", s.id, target.id)),
                Duration::ZERO,
                10.0,
            ));
        }
        section.into_any_element()
    }

    /// The server's shared channels, both ends, each of which can be ended. None while it has none.
    fn shares_list(&self, id: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shares = self.servers.detail.shares.as_ref().filter(|s| !s.is_empty())?;
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, c) in shares.iter().enumerate() {
            let waiting = c.state() == pb::SharedConnectionState::Waiting;
            let other = c
                .server
                .as_ref()
                .map(|o| pb::Server {
                    id: o.id.clone(),
                    name: o.name.clone(),
                    icon_url: o.icon_url.clone(),
                    ..Default::default()
                })
                .unwrap_or_default();
            let (channel, server) = (channel_of(c), other_of(c));
            let text = if c.home {
                t_with(
                    "instancesettings.servers.shownIn",
                    &[("channel", Arg::Str(&channel)), ("server", Arg::Str(&server))],
                )
            } else {
                t_with(
                    "instancesettings.servers.from",
                    &[("channel", Arg::Str(&channel)), ("server", Arg::Str(&server))],
                )
            };
            let ending = c.clone();
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(corner(12.0))
                    .bg(alpha(p.muted, 0.5))
                    .child(
                        div().relative().flex_none().child(server_icon(&other, 32.0, 10.0, p)).child(
                            div()
                                .absolute()
                                .right(px(-4.0))
                                .bottom(px(-4.0))
                                .size(px(16.0))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(p.background)
                                .text_color(p.primary)
                                .child(icon("link-2").size(px(10.0))),
                        ),
                    )
                    .child(div().flex_1().min_w_0().text_sm().text_ellipsis().child(text))
                    .child(if waiting {
                        pill(&t("serversettings.sharedChannels.waiting").to_uppercase(), amber(p))
                    } else {
                        pill(&t("chat.shared.shared").to_uppercase(), green())
                    })
                    .child(
                        soft_button(SharedString::from(format!("share-end-{}", c.id)), "", p)
                            .text_color(p.destructive)
                            .child(icon("unlink").size(px(15.0)))
                            .child(t("instancesettings.servers.end"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.servers.detail.ending = Some(ending.clone());
                                this.servers.detail.ending_error = None;
                                cx.notify();
                            })),
                    ),
                SharedString::from(format!("share-{id}-{}", c.id)),
                Duration::from_millis(40 * n as u64),
                8.0,
            ));
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(heading(Some("link-2"), &t("serversettings.nav.shared"), p))
                .child(list)
                .into_any_element(),
        )
    }

    fn server_actions(
        &mut self,
        entry: &pb::InstanceServer,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = entry.server.clone().unwrap_or_default();
        let id = s.id.clone();
        let progress = self
            .servers
            .export
            .as_ref()
            .filter(|e| e.server == id)
            .map(|e| f32::from_bits(e.progress.load(Ordering::Relaxed)));
        let busy_elsewhere = self.servers.export.as_ref().is_some_and(|e| e.server != id);
        let done =
            progress.is_none() && self.servers.exported.is_some_and(|at| at.elapsed() < Duration::from_millis(2200));
        let fill =
            progress.map(|f| motion::follow(SharedString::from(format!("export-{id}")), f.max(0.04), window, cx));
        let save = div()
            .id("server-export")
            .relative()
            .h(px(36.0))
            .px(px(14.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(corner(10.0))
            .overflow_hidden()
            .border_1()
            .border_color(p.border)
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .when(busy_elsewhere, |el| el.opacity(0.6))
            .hover({
                let c = alpha(p.primary, 0.08);
                move |s| s.bg(c)
            })
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(|this, _, window, cx| this.export_open_server(window, cx)))
            .when_some(fill, |el, f| {
                el.child(div().absolute().top_0().bottom_0().left_0().w(relative(f)).bg(alpha(p.primary, 0.2)))
            })
            .child(motion::once(
                div().relative().child(match (done, progress.is_some()) {
                    (true, _) => icon("check").size(px(15.0)).text_color(green()).into_any_element(),
                    (_, true) => spinner(SharedString::from(format!("export-spin-{id}")), 15.0, window),
                    _ => icon("download").size(px(15.0)).into_any_element(),
                }),
                SharedString::from(format!("export-glyph-{}-{}", done, progress.is_some())),
                Duration::from_millis(300),
                |el, t| el.opacity(t),
            ))
            .child(div().relative().child(match progress {
                Some(f) => t_with(
                    "instancesettings.servers.saving",
                    &[("percent", Arg::Str(&format!("{}%", (f * 100.0).round() as u32)))],
                ),
                None => t("instancesettings.servers.saveFile"),
            }));
        let deleting = self.servers.detail.deleting;
        let mut row = div().flex().items_center().gap(px(8.0)).pt(px(12.0)).border_t_1().border_color(p.border);
        if entry.member {
            let open = id.clone();
            row = row
                .child(icon_soft("server-open", "arrow-right", t("instancesettings.servers.openIt"), p).on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(InstanceSettingsEvent::OpenServer(open.clone()))),
                ));
        }
        row = row.child(save).child(div().flex_1()).child(
            soft_button("server-delete", "", p)
                .text_color(p.destructive)
                .when(deleting, |el| el.bg(alpha(p.destructive, 0.1)))
                .child(icon("trash").size(px(15.0)))
                .child(t("serversettings.shared.delete"))
                .on_click(cx.listener(|this, _, window, cx| {
                    let d = &mut this.servers.detail;
                    d.deleting = !d.deleting;
                    d.delete_error = None;
                    if d.deleting {
                        let confirm = this.servers.confirm.clone();
                        confirm.update(cx, |s, cx| {
                            s.set_value("", window, cx);
                            s.focus(window, cx);
                        });
                    }
                    cx.notify();
                })),
        );
        let mut section = div().flex().flex_col().gap(px(12.0)).child(row).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .text_color(p.muted_foreground)
                .child(icon("lock-open").size(px(13.0)))
                .child(t("desktop.instance.fileNotEncrypted")),
        );
        if deleting {
            let typed = self.servers.confirm.read(cx).value().to_string();
            let armed = typed == s.name;
            let d = &self.servers.detail;
            let busy = d.delete_busy;
            section = section.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(corner(16.0))
                    .border_1()
                    .border_color(alpha(p.destructive, 0.4))
                    .bg(alpha(p.destructive, 0.05))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.destructive)
                            .child(icon("triangle-alert").size(px(16.0)))
                            .child(t_with("serversettings.danger.title", &[("server", Arg::Str(&s.name))])),
                    )
                    .child(
                        div().text_sm().text_color(p.muted_foreground).child(t("instancesettings.servers.deleteHint")),
                    )
                    .child(div().text_sm().child(marked(
                        &t_with("serversettings.danger.confirm", &[("name", Arg::Str(&strong(&s.name)))]),
                        p,
                    )))
                    .child(
                        div()
                            .rounded(corner(12.0))
                            .border_2()
                            .border_color(if armed {
                                alpha(p.destructive, 0.35)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .child(Input::new(&self.servers.confirm)),
                    )
                    .when_some(error_line(d.delete_error.as_deref(), p), |el, e| el.child(e))
                    .child(
                        div().flex().justify_end().child(motion::once(
                            danger_button(
                                "server-delete-go",
                                if busy {
                                    t("accountsettings.privacy.deleting")
                                } else {
                                    t("serversettings.nav.danger")
                                },
                                p,
                            )
                            .when(!armed || busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, window, cx| this.delete_open_server(window, cx))),
                            SharedString::from(format!("server-delete-armed-{armed}")),
                            Duration::from_millis(400),
                            move |el, t| {
                                if armed {
                                    // A little shake once it's ready.
                                    let wiggle = (t * std::f32::consts::TAU * 2.0).sin() * (1.0 - t);
                                    el.relative().left(px(wiggle * 3.0))
                                } else {
                                    el
                                }
                            },
                        )),
                    ),
                SharedString::from(format!("server-delete-{id}")),
                Duration::ZERO,
                10.0,
            ));
        }
        section.into_any_element()
    }

    /// Confirms ending a shared channel, over the page.
    pub(super) fn share_dialog(&mut self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = &self.servers.detail;
        let c = d.ending.clone()?;
        let busy = d.ending_busy;
        let channel = channel_of(&c);
        let ask = if c.state() == pb::SharedConnectionState::Waiting {
            t_with("instancesettings.servers.endRequestAsk", &[("channel", Arg::Str(&channel))])
        } else {
            t_with("instancesettings.servers.endSharingAsk", &[("channel", Arg::Str(&channel))])
        };
        let body = if c.home {
            t_with("instancesettings.servers.endHomeBody", &[("server", Arg::Str(&other_of(&c)))])
        } else {
            t_with("instancesettings.servers.endGuestBody", &[("server", Arg::Str(&other_of(&c)))])
        };
        let panel = card(p)
            .w(px(440.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.0))
                    .child(
                        div()
                            .size(px(44.0))
                            .flex_none()
                            .rounded(corner(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.destructive, 0.14))
                            .text_color(p.destructive)
                            .child(icon("unlink").size(px(22.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(ask))
                            .child(div().text_sm().text_color(p.muted_foreground).child(body)),
                    ),
            )
            .when_some(error_line(d.ending_error.as_deref(), p), |el, e| el.child(e))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(10.0))
                    .child(soft_button("share-dialog-cancel", t("common.cancel"), p).on_click(cx.listener(
                        |this, _, _, cx| {
                            if !this.servers.detail.ending_busy {
                                this.servers.detail.ending = None;
                                cx.notify();
                            }
                        },
                    )))
                    .child(
                        danger_button(
                            "share-dialog-ok",
                            if busy { t("desktop.instance.ending") } else { t("instancesettings.servers.endIt") },
                            p,
                        )
                        .when(busy, |el| el.opacity(0.7))
                        .on_click(cx.listener(|this, _, window, cx| this.end_share(window, cx))),
                    ),
            );
        Some(
            motion::fade_in(
                crate::ui::overlay::scrim("share-scrim", p)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.servers.detail.ending_busy {
                            this.servers.detail.ending = None;
                            cx.notify();
                        }
                    }))
                    .child(motion::rise(
                        div().id("share-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
                        SharedString::from(format!("share-dialog-{}", c.id)),
                        Duration::ZERO,
                        24.0,
                    )),
                SharedString::from(format!("share-dialog-fade-{}", c.id)),
                Duration::from_millis(180),
            )
            .into_any_element(),
        )
    }
}

fn channel_of(c: &pb::SharedConnection) -> String {
    if c.home_channel_name.is_empty() {
        format!("#{}", t("instancesettings.servers.aChannel"))
    } else {
        format!("#{}", c.home_channel_name)
    }
}

fn other_of(c: &pb::SharedConnection) -> String {
    c.server
        .as_ref()
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| t("serversettings.sharedChannels.anotherServer"))
}

/// Biggest, Members or Newest, with a highlight that glides to the pick.
fn sorter(value: Sort, p: &Palette, window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> AnyElement {
    let tabs = [
        (Sort::Biggest, t("instancesettings.servers.biggest")),
        (Sort::Members, t("serversettings.nav.members")),
        (Sort::Newest, t("instancesettings.servers.newest")),
    ];
    let widths: Vec<f32> = tabs.iter().map(|(_, label)| 24.0 + 7.4 * label.chars().count() as f32).collect();
    let at = tabs.iter().position(|(s, _)| *s == value).unwrap_or(0);
    let left: f32 = widths[..at].iter().sum::<f32>() + 4.0;
    let x = motion::follow("servers-sort-x", left, window, cx);
    let w = motion::follow("servers-sort-w", widths[at], window, cx);
    let mut row = div()
        .relative()
        .flex()
        .flex_none()
        .h(px(40.0))
        .p(px(4.0))
        .rounded(corner(12.0))
        .bg(p.secondary)
        .child(div().absolute().top(px(4.0)).bottom(px(4.0)).left(px(x)).w(px(w)).rounded(corner(9.0)).bg(p.card));
    for ((sort, label), width) in tabs.into_iter().zip(widths) {
        let on = sort == value;
        row = row.child(
            div()
                .id(SharedString::from(format!("servers-sort-{sort:?}")))
                .relative()
                .w(px(width))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .cursor_pointer()
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.servers.sort != sort {
                        this.servers.sort = sort;
                        this.servers.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
                        cx.notify();
                    }
                }))
                .child(label),
        );
    }
    row.into_any_element()
}
