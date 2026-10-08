//! The Servers page: every community server on the instance, with its owner
//! and what it holds. Admins open one to change its caps, move it to another
//! region, end its shared channels, save its whole file or delete it,
//! whether or not they're in it. The web's `settings/instance/Servers.tsx`:
//! a row opens in place, one at a time.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
    relative,
};

use super::controls::{cap_row, dialog, dialog_buttons, input_box, segmented, shimmer};
use super::{InstanceSettingsEvent, InstanceSettingsView};
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin, count_label, format_bytes, size_label};
use crate::core::instance_manage as manage;
use crate::core::instance_servers::{self as servers, CAPS, Sort, members_of, storage_of};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{icon, server_icon};

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
            cx.subscribe(&query, |_: &mut InstanceSettingsView, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
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
    count_label(Some(n.round() as i64))
}

fn bytes(n: f64) -> String {
    format_bytes(n.round() as i64)
}

/// A small heading over a part of the page (`text-xs font-bold tracking-wide uppercase`).
fn heading(glyph: Option<&str>, text: &str, p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .when_some(glyph, |el, g| el.child(icon(g).size(px(14.0))))
        .child(text.to_uppercase())
}

/// One of the totals over the list (`rounded-2xl border bg-background/50 p-3`), counting up.
fn total(id: String, glyph: &str, label: &str, value: f64, sized: bool, n: usize, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex_1()
            .min_w_0()
            .p(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(10.4))
                    .line_height(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(icon(glyph).size(px(14.0)))
                    .child(label.to_uppercase()),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .text_xl()
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_ellipsis()
                    .child(motion::count_up(
                        SharedString::from(format!("{id}-count")),
                        value,
                        Duration::from_millis(100 + 50 * n as u64),
                        if sized { bytes } else { group },
                    )),
            ),
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
        let place = servers::region_name(&regions, &to);
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
                        this.toast(
                            "unlink",
                            t_with(
                                "instancesettings.servers.ended",
                                &[
                                    ("channel", Arg::Str(&channel_of(&ending))),
                                    ("server", Arg::Str(&other_of(&ending))),
                                ],
                            ),
                            cx,
                        );
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
                        this.toast(
                            "download",
                            t_with(
                                "instancesettings.servers.savedFile",
                                &[("file", Arg::Str(&name)), ("size", Arg::Str(&format_bytes(size as i64)))],
                            ),
                            cx,
                        );
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
            return div().text_sm().text_color(p.muted_foreground).child(capitalized(error)).into_any_element();
        }
        if self.servers.list.is_none() {
            return div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(div().flex().gap(px(8.0)).children((0..4).map(|n| div().flex_1().child(shimmer(n, 80.0, p)))))
                .children((0..3).map(|n| shimmer(4 + n, 64.0, p)))
                .into_any_element();
        }
        self.server_list(p, window, cx)
    }

    fn server_list(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let list = self.servers.list.clone().unwrap_or_default();
        let memberships: i64 = list.iter().map(members_of).sum();
        let storage: i64 = list.iter().map(storage_of).sum();
        let (pictures, picture_bytes) = self.servers.pictures;
        let pictures_sub = t_with("instancesettings.servers.picturesSub", &[("count", Arg::Num(pictures))]);
        let totals = div()
            .flex()
            .gap(px(8.0))
            .child(total(
                "servers-total-servers".into(),
                "server",
                &t("instancesettings.nav.servers"),
                list.len() as f64,
                false,
                0,
                p,
            ))
            .child(total(
                "servers-total-members".into(),
                "users",
                &t("instancesettings.servers.memberships"),
                memberships as f64,
                false,
                1,
                p,
            ))
            .child(total(
                "servers-total-storage".into(),
                "hard-drive",
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
                        gpui_kit::component::tooltip::Tooltip::new(pictures_sub.clone()).build(window, cx)
                    })
                    .child(total(
                        format!("servers-total-pictures-{picture_bytes}"),
                        "image",
                        &t("instancesettings.servers.pictures"),
                        picture_bytes as f64,
                        true,
                        3,
                        p,
                    )),
            );

        let query = self.servers.query.read(cx).value().to_string();
        let sort = self.servers.sort;
        let shown: Vec<pb::InstanceServer> = servers::shown(&list, &query, sort).into_iter().cloned().collect();
        let biggest = list.iter().map(storage_of).max().unwrap_or(0).max(1);
        let regions = self.regions();
        let sorts = [Sort::Biggest, Sort::Members, Sort::Newest];
        let picker = segmented(
            "servers-sort",
            vec![
                t("instancesettings.servers.biggest"),
                t("serversettings.nav.members"),
                t("instancesettings.servers.newest"),
            ],
            sorts.iter().position(|s| *s == sort).unwrap_or(0),
            p,
            window,
            cx,
            move |this, n, _, cx| {
                this.servers.sort = sorts[n];
                cx.notify();
            },
        );
        let top = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().flex_1().min_w_0().child(focus_ring(
                input_box(Input::new(&self.servers.query).appearance(false), Some("search"), p),
                has_focus(&self.servers.query, window, cx),
                p,
            )))
            .child(picker);
        let count = shown.len();
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(icon("server").size(px(14.0)))
            .child(t_with("instancesettings.servers.count", &[("count", Arg::Num(count as i64))]).to_uppercase());
        let mut rows = div().flex().flex_col().gap(px(6.0));
        let now = now_ms();
        for (n, entry) in shown.iter().enumerate() {
            rows = rows.child(self.server_row(entry, n, biggest, &regions, now, p, window, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(totals)
            .child(top)
            .child(header)
            .child(rows)
            .when(count == 0, |el| {
                el.child(motion::rise(
                    div().py(px(32.0)).flex().justify_center().text_sm().text_color(p.muted_foreground).child(t(
                        if query.trim().is_empty() {
                            "instancesettings.servers.none"
                        } else {
                            "instancesettings.servers.noMatch"
                        },
                    )),
                    SharedString::from(format!("servers-none-{}", query.trim().is_empty())),
                    Duration::ZERO,
                    8.0,
                ))
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn server_row(
        &mut self,
        entry: &pb::InstanceServer,
        index: usize,
        biggest: i64,
        regions: &[pb::Region],
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(s) = entry.server.as_ref() else { return div().into_any_element() };
        let id = s.id.clone();
        let open = self.servers.open.as_deref() == Some(id.as_str());
        let storage = storage_of(entry);
        let cap = entry.limits.as_ref().and_then(|l| l.storage_bytes);
        let share = match cap {
            Some(cap) if cap > 0 => (storage as f32 / cap as f32).min(1.0),
            _ => storage as f32 / biggest as f32,
        };
        let full = cap.is_some() && share >= 0.9;
        let day =
            s.created_at.as_ref().map(|t| manage::day_label(t.seconds * 1000, now).to_lowercase()).unwrap_or_default();
        let byline = match entry.owner.as_ref() {
            Some(o) => t_with(
                "instancesettings.servers.byline",
                &[("name", Arg::Str(&user_name(o))), ("username", Arg::Str(&o.username)), ("day", Arg::Str(&day))],
            ),
            None => t_with("instancesettings.servers.bylineNobody", &[("day", Arg::Str(&day))]),
        };
        let amber_500: Hsla = gpui_kit::rgb(0xfe9a00).into();
        let bar_color: Hsla = if full { amber_500 } else { alpha(p.primary, 0.7) };
        let line = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .child(
                div()
                    .min_w_0()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .line_height(px(24.0))
                    .font_weight(FontWeight::BOLD)
                    .child(s.name.clone()),
            )
            .child(icon(if s.discoverable { "globe" } else { "eye-off" }).size(px(14.0)).text_color(p.muted_foreground))
            .when(servers::has_regions(regions), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(2.0))
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded_full()
                        .bg(p.muted)
                        .text_color(p.muted_foreground)
                        .text_size(px(10.4))
                        .line_height(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .child(icon("map-pin").size(px(10.0)))
                        .child(servers::region_name(regions, &s.region)),
                )
            })
            .when(entry.member, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded_full()
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .text_size(px(10.4))
                        .line_height(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .child(t("instancesettings.servers.youreIn").to_uppercase()),
                )
            });
        let numbers = div()
            .flex()
            .flex_none()
            .flex_col()
            .items_end()
            .text_xs()
            .line_height(px(16.0))
            .text_color(p.muted_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("users").size(px(12.0)))
                    .child(count_label(Some(members_of(entry)))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .when(full, |el| el.text_color(amber_500).font_weight(FontWeight::BOLD))
                    .child(icon("hard-drive").size(px(12.0)))
                    .child(format_bytes(storage))
                    .when_some(cap, |el, cap| {
                        el.child(
                            div().text_color(alpha(p.muted_foreground, 0.7)).child(format!("/ {}", format_bytes(cap))),
                        )
                    }),
            );
        let delay = Duration::from_millis(100 + 30 * index.min(14) as u64);
        // `absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-muted/60`, filling once.
        let bar = div()
            .absolute()
            .left(px(12.0))
            .right(px(12.0))
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
        let turn = motion::follow(
            SharedString::from(format!("server-chevron-{id}")),
            if open { 1.0 } else { 0.0 },
            window,
            cx,
        );
        let toggle = id.clone();
        let head = div()
            .id(SharedString::from(format!("server-{id}")))
            .relative()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(10.0))
            .pr(px(12.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.servers.open.as_deref() == Some(toggle.as_str()) {
                    this.close_server(cx);
                } else {
                    this.open_server(toggle.clone(), window, cx);
                }
            }))
            .child(server_icon(s, 40.0, 20.0, p))
            .child(
                div().flex_1().min_w_0().flex().flex_col().child(line).child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(byline),
                ),
            )
            .child(numbers)
            .child(
                icon("chevron-down")
                    .size(px(16.0))
                    .text_color(p.muted_foreground)
                    .rotate(gpui_kit::radians(turn * std::f32::consts::PI)),
            )
            .child(bar);
        let (hover_edge, hover_bg) = (alpha(p.primary, 0.3), alpha(p.muted, 0.4));
        let row = div()
            .id(SharedString::from(format!("server-card-{id}")))
            .rounded(radius_2xl())
            .border_1()
            .bg(alpha(p.background, 0.4))
            .map(|el| {
                if open {
                    el.border_color(alpha(p.primary, 0.4))
                } else {
                    el.border_color(p.border).hover(move |s| s.border_color(hover_edge).bg(hover_bg))
                }
            })
            .child(head)
            .when(open, |el| el.child(self.server_details(entry, p, window, cx)));
        motion::rise(
            row,
            SharedString::from(format!("server-in-{id}")),
            Duration::from_millis(20 * index.min(14) as u64),
            10.0,
        )
        .into_any_element()
    }

    /// A server opened up: what it holds, its caps, and what an admin can do with it (`Details`).
    fn server_details(
        &mut self,
        entry: &pb::InstanceServer,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = entry.server.clone().unwrap_or_default();
        let id = s.id.clone();
        let regions = self.regions();
        let u = entry.usage.clone().unwrap_or_default();
        let stat = |n: usize, key: &str, value: f64, sized: bool| {
            motion::rise(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.muted, 0.5))
                    .child(
                        div()
                            .text_size(px(10.4))
                            .line_height(px(14.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(t(key).to_uppercase()),
                    )
                    .child(div().text_lg().line_height(px(28.0)).font_weight(FontWeight::EXTRA_BOLD).child(
                        motion::count_up(
                            SharedString::from(format!("server-{id}-{key}")),
                            value,
                            Duration::from_millis(100 + 40 * n as u64),
                            if sized { bytes } else { group },
                        ),
                    )),
                SharedString::from(format!("server-{id}-stat-{n}")),
                Duration::from_millis(50 + 40 * n as u64),
                8.0,
            )
        };
        let stats = div()
            .flex()
            .gap(px(8.0))
            .child(stat(0, "serversettings.nav.members", u.members as f64, false))
            .child(stat(1, "serversettings.nav.channels", u.channels as f64, false))
            .child(stat(2, "serversettings.usage.messages", u.messages as f64, false))
            .child(stat(3, "serversettings.limits.files", u.attachment_bytes as f64, true));
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .p(px(16.0))
            .border_t_1()
            .border_color(alpha(p.border, 0.6))
            .when(!s.description.is_empty(), |el| {
                el.child(
                    div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(s.description.clone()),
                )
            })
            .child(stats)
            .child(self.server_caps(&id, p, window, cx));
        if servers::has_regions(&regions) {
            body = body.child(self.region_picker(&s, &regions, p, window, cx));
        }
        if let Some(shares) = self.shares_list(&id, p, cx) {
            body = body.child(shares);
        }
        body = body.child(self.server_actions(entry, p, window, cx));
        motion::rise(body, SharedString::from(format!("server-open-{id}")), Duration::ZERO, 8.0).into_any_element()
    }

    fn server_caps(&mut self, id: &str, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut section =
            div().flex().flex_col().gap(px(12.0)).child(heading(None, &t("instancesettings.servers.caps"), p));
        if self.servers.detail.own.is_none() {
            return section.child(shimmer(0, 160.0, p)).into_any_element();
        }
        for (n, (_, sized, read, _)) in CAPS.into_iter().enumerate() {
            let value = read(&self.servers.detail.draft);
            let fallback = read(&self.servers.defaults);
            let default = t_with(
                "instancesettings.servers.defaultIs",
                &[("value", Arg::Str(&if sized { size_label(fallback) } else { count_label(fallback) }))],
            );
            let state = self.servers.caps[n].clone();
            let unit = sized.then(|| self.servers.units[n]);
            section = section.child(cap_row(
                &format!("scap-{id}-{n}"),
                &t(CAP_KEYS[n]),
                value.is_some(),
                Some(&default),
                Some(&state),
                unit,
                p,
                window,
                cx,
                move |this, on, window, cx| this.switch_server_cap(n, on, window, cx),
                move |this, k, cx| this.pick_server_unit(n, k, cx),
            ));
        }
        let changed = self.caps_changed();
        let d = &self.servers.detail;
        let saving = d.saving;
        if let Some(e) = &d.save_error {
            section = section.child(div().text_sm().text_color(p.destructive).child(capitalized(e)));
        }
        if changed > 0 {
            section = section.child(motion::rise(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        button("scap-discard", t("settings.controls.discard"), None, Look::Ghost, true, p)
                            .rounded(radius_xl())
                            .when(saving, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, window, cx| this.discard_server_caps(window, cx))),
                    )
                    .child(
                        button("scap-save", t("instancesettings.servers.saveCaps"), None, Look::Primary, true, p)
                            .rounded(radius_xl())
                            .px(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .when(saving, |el| el.opacity(0.5).child(spinner("scap-spin", 16.0, window)))
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
                    .py(px(6.0))
                    .pl(px(6.0))
                    .pr(px(12.0))
                    .rounded_full()
                    .border_1()
                    .border_color(edge)
                    .text_sm()
                    .line_height(px(20.0))
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
                            .text_size(px(9.6))
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
            let here = servers::region_name(regions, &s.region);
            let to = target.name.clone();
            let plane = icon("plane").size(px(16.0)).text_color(p.primary);
            // The plane waits partway, and flies across while the server travels.
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
                    .rounded(radius_2xl())
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
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(t_with(
                                "instancesettings.servers.moveWhat",
                                &[("to", Arg::Str(&to)), ("from", Arg::Str(&here))],
                            ))
                            .child(t("instancesettings.servers.moveWait")),
                    )
                    .when_some(d.move_error.clone(), |el, e| {
                        el.child(div().text_sm().text_color(p.destructive).child(capitalized(&e)))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                button("region-cancel", t("common.cancel"), None, Look::Ghost, true, p)
                                    .rounded(radius_xl())
                                    .when(moving, |el| el.opacity(0.5))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.servers.detail.moving {
                                            this.servers.detail.move_to = None;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                button(
                                    "region-go",
                                    t_with(
                                        if moving {
                                            "instancesettings.servers.moving"
                                        } else {
                                            "instancesettings.servers.moveTo"
                                        },
                                        &[("region", Arg::Str(&to))],
                                    ),
                                    if moving { None } else { Some("plane") },
                                    Look::Primary,
                                    true,
                                    p,
                                )
                                .rounded(radius_xl())
                                .px(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .when(moving, |el| el.opacity(0.5).child(spinner("region-spin", 16.0, window)))
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
            let template = t(if c.home { "instancesettings.servers.shownIn" } else { "instancesettings.servers.from" });
            let text = bold_parts(&template, &[("channel", channel.as_str()), ("server", server.as_str())]);
            let ending = c.clone();
            // The web's `StateChip`.
            let chip = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(2.0))
                .rounded_full()
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .map(|el| {
                    if waiting {
                        el.bg(p.muted)
                            .text_color(p.muted_foreground)
                            .child(div().size(px(8.0)).rounded_full().bg(p.primary))
                    } else {
                        el.bg(alpha(p.primary, 0.12)).text_color(p.primary).child(icon("check").size(px(12.0)))
                    }
                })
                .child(t(if waiting {
                    "serversettings.sharedChannels.waiting"
                } else {
                    "serversettings.sharedChannels.connected"
                }));
            list = list.child(motion::rise(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(12.0))
                    .gap_y(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.muted, 0.5))
                    .child(
                        div().relative().flex_none().child(server_icon(&other, 32.0, 16.0, p)).child(
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
                    .child(div().flex_1().min_w(px(160.0)).text_sm().line_height(px(20.0)).child(text))
                    .child(chip)
                    .child(
                        button(
                            SharedString::from(format!("share-end-{}", c.id)),
                            t("instancesettings.servers.end"),
                            Some("unlink"),
                            Look::DangerOutline,
                            true,
                            p,
                        )
                        // Ghost in red: the danger outline's hover without its border.
                        .border_0()
                        .bg(alpha(p.destructive, 0.0))
                        .shadow(Vec::new())
                        .rounded(radius_xl())
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
                .gap(px(8.0))
                .child(heading(None, &t("serversettings.nav.shared"), p))
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
        let glyph = match (done, progress.is_some()) {
            (true, _) => icon("check").size(px(16.0)).text_color(gpui_kit::rgb(0x00bc7d)).into_any_element(),
            (_, true) => spinner(SharedString::from(format!("export-spin-{id}")), 16.0, window),
            _ => icon("download").size(px(16.0)).into_any_element(),
        };
        let label = match progress {
            Some(f) => {
                let percent = format!("{}%", (f * 100.0).round() as u32);
                t_with("instancesettings.servers.saving", &[("percent", Arg::Str(&percent))])
            }
            None => t("instancesettings.servers.saveFile"),
        };
        let save = button("server-export", "", None, Look::Outline, false, p)
            .relative()
            .overflow_hidden()
            .rounded(radius_xl())
            .when(busy_elsewhere || progress.is_some(), |el| el.opacity(0.6))
            .on_click(cx.listener(|this, _, window, cx| this.export_open_server(window, cx)))
            .when_some(fill, |el, f| {
                el.child(div().absolute().top_0().bottom_0().left_0().w(relative(f)).bg(alpha(p.primary, 0.2)))
            })
            .child(motion::once(
                div().relative().child(glyph),
                SharedString::from(format!("export-glyph-{}-{}", done, progress.is_some())),
                Duration::from_millis(300),
                |el, t| el.opacity(t),
            ))
            .child(div().relative().child(label));
        let deleting = self.servers.detail.deleting;
        let red = alpha(p.destructive, 0.1);
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .pt(px(12.0))
            .border_t_1()
            .border_color(alpha(p.border, 0.6));
        if entry.member {
            let open = id.clone();
            row = row.child(
                button("server-open", t("instancesettings.servers.openIt"), None, Look::Outline, false, p)
                    .rounded(radius_xl())
                    .child(icon("arrow-right").size(px(16.0)))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(InstanceSettingsEvent::OpenServer(open.clone())))),
            );
        }
        row = row.child(save).child(div().flex_1()).child(
            button("server-delete", t("serversettings.shared.delete"), Some("trash-2"), Look::DangerOutline, false, p)
                // Ghost in red: the danger outline's hover without its border.
                .border_0()
                .shadow(Vec::new())
                .rounded(radius_xl())
                .bg(if deleting { red } else { alpha(p.destructive, 0.0) })
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
        let mut section = div().flex().flex_col().gap(px(16.0)).child(row);
        if deleting {
            let typed = self.servers.confirm.read(cx).value().to_string();
            let armed = typed == s.name;
            let d = &self.servers.detail;
            let busy = d.delete_busy;
            let confirm_text = bold_parts(
                &t_with("serversettings.danger.confirm", &[("name", Arg::Str("{name}"))]),
                &[("name", s.name.as_str())],
            );
            section = section.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(radius_2xl())
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
                    .child(div().text_sm().child(confirm_text))
                    .child(
                        focus_ring(
                            input_box(Input::new(&self.servers.confirm).appearance(false), None, p),
                            has_focus(&self.servers.confirm, window, cx),
                            p,
                        )
                        .when(armed, |el| el.border_color(p.destructive)),
                    )
                    .when_some(d.delete_error.clone(), |el, e| {
                        el.child(div().text_sm().text_color(p.destructive).child(capitalized(&e)))
                    })
                    .child(
                        div().flex().justify_end().child(motion::once(
                            button(
                                "server-delete-go",
                                t("serversettings.nav.danger"),
                                if busy { None } else { Some("trash-2") },
                                Look::Destructive,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .when(busy, |el| el.child(spinner("server-delete-spin", 16.0, window)))
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

    /// Confirms ending a shared channel, over the page (the web's `ConfirmDialog`).
    pub(super) fn share_dialog(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let d = &self.servers.detail;
        let c = d.ending.clone()?;
        let busy = d.ending_busy;
        let (channel, other) = (channel_of(&c), other_of(&c));
        let title = t_with(
            if c.state() == pb::SharedConnectionState::Waiting {
                "instancesettings.servers.endRequestAsk"
            } else {
                "instancesettings.servers.endSharingAsk"
            },
            &[("channel", Arg::Str(&channel))],
        );
        let description = t_with(
            if c.home { "instancesettings.servers.endHomeBody" } else { "instancesettings.servers.endGuestBody" },
            &[("server", Arg::Str(&other))],
        );
        let body = div()
            .flex()
            .flex_col()
            .when_some(d.ending_error.clone(), |el, e| {
                el.child(div().mb(px(12.0)).text_sm().text_color(p.destructive).child(capitalized(&e)))
            })
            .child(
                dialog_buttons()
                    .child(
                        button("share-dialog-cancel", t("serversettings.shared.keepIt"), None, Look::Ghost, false, p)
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.servers.detail.ending_busy {
                                    this.servers.detail.ending = None;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        button(
                            "share-dialog-ok",
                            t("instancesettings.servers.endIt"),
                            None,
                            Look::Destructive,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .when(busy, |el| el.opacity(0.5).child(spinner("share-dialog-spin", 16.0, window)))
                        .on_click(cx.listener(|this, _, window, cx| this.end_share(window, cx))),
                    ),
            );
        Some(dialog(&format!("share-dialog-{}", c.id), title, Some(description), body, p, cx, |this, cx| {
            if !this.servers.detail.ending_busy {
                this.servers.detail.ending = None;
                cx.notify();
            }
        }))
    }
}

/// The caps' names, in `CAPS` order.
const CAP_KEYS: [&str; 6] = [
    "serversettings.nav.members",
    "serversettings.nav.channels",
    "serversettings.usage.storage",
    "serversettings.limits.files",
    "serversettings.nav.emoji",
    "serversettings.nav.recordings",
];

fn channel_of(c: &pb::SharedConnection) -> String {
    let name = if c.home_channel_name.is_empty() {
        t("instancesettings.servers.aChannel")
    } else {
        c.home_channel_name.clone()
    };
    format!("#{name}")
}

fn other_of(c: &pb::SharedConnection) -> String {
    c.server
        .as_ref()
        .map(|s| s.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| t("serversettings.sharedChannels.anotherServer"))
}

/// A sentence with its placeholders filled in bold.
fn bold_parts(template: &str, parts: &[(&str, &str)]) -> gpui_kit::StyledText {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        text.push_str(&rest[..open]);
        let name = &rest[open + 1..open + close];
        match parts.iter().find(|(n, _)| *n == name) {
            Some((_, value)) => {
                let start = text.len();
                text.push_str(value);
                runs.push((
                    start..text.len(),
                    gpui_kit::HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
                ));
            }
            None => text.push_str(&rest[open..open + close + 1]),
        }
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    gpui_kit::StyledText::new(text).with_highlights(runs)
}

/// An instance's message with its first letter up, as the web shows them.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
