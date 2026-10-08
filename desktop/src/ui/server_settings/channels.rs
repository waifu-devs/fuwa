//! The Channels page: every channel in order, dragged by its handle (into
//! and out of categories too), and the chosen one beside the
//! list: its name, topic, category and slow mode, and who can see and do
//! what in it. The web's `settings/server/Channels.tsx` and
//! `ChannelPermissions.tsx`.

use gpui_kit::base::{Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack};
use gpui_kit::component::slider::{SliderEvent, SliderState};

use super::roles::{dot, group_name, member_name, permission_here, permission_name, role_color};
use gpui_kit::{Render, Stateful};

use super::menu::Item;
use super::pages::form_row;
use super::*;
use crate::core::arrange::{self, Layout};
use crate::core::permissions::{self, Access, Bits, CHANNEL_GROUPS, bit};
use crate::core::server_admin::ChannelPatch;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{radius_2xl, radius_lg, radius_md, radius_xl};

/// Discord's slow mode stops, in seconds.
const SLOW: [i32; 14] = [0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];
/// How tall a channel row is, and the gap over a category.
const ROW: f32 = 34.0;
const CATEGORY_GAP: f32 = 8.0;
const VIEW: Bits = bit(P::ViewChannels);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Permissions,
    Share,
}

/// One role's or person's rules in a channel, as bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Over {
    target_id: String,
    member: bool,
    allow: Bits,
    deny: Bits,
}

fn from_wire(list: &[pb::PermissionOverwrite]) -> Vec<Over> {
    list.iter()
        .map(|o| Over {
            target_id: o.target_id.clone(),
            member: o.target == pb::OverwriteTarget::Member as i32,
            allow: permissions::from_list(&o.allow),
            deny: permissions::from_list(&o.deny),
        })
        .collect()
}

fn to_wire(list: &[Over]) -> Vec<pb::PermissionOverwrite> {
    list.iter()
        .filter(|o| o.allow != 0 || o.deny != 0)
        .map(|o| pb::PermissionOverwrite {
            target_id: o.target_id.clone(),
            target: if o.member { pb::OverwriteTarget::Member } else { pb::OverwriteTarget::Role } as i32,
            allow: permissions::to_list(o.allow),
            deny: permissions::to_list(o.deny),
        })
        .collect()
}

/// The overwrites that do something, in a fixed order, to compare two lists.
fn canon(list: &[Over]) -> Vec<(String, bool, Bits, Bits)> {
    let mut out: Vec<_> = list
        .iter()
        .filter(|o| o.allow != 0 || o.deny != 0)
        .map(|o| (o.target_id.clone(), o.member, o.allow, o.deny))
        .collect();
    out.sort();
    out
}

/// How many roles or people a draft changes.
fn changed(base: &[Over], draft: &[Over]) -> usize {
    let get = |list: &[Over], id: &str| list.iter().find(|o| o.target_id == id).map(|o| (o.allow, o.deny));
    let mut ids: Vec<&str> = base.iter().chain(draft).map(|o| o.target_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter().filter(|id| get(base, id).unwrap_or((0, 0)) != get(draft, id).unwrap_or((0, 0))).count()
}

/// Channel names are lowercase with dashes, like the server makes them.
pub(super) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_whitespace() {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else if c.is_alphanumeric() || c == '_' || c == '-' {
            out.extend(c.to_lowercase());
        }
    }
    out.chars().take(100).collect()
}

fn slow_label(seconds: i32) -> String {
    if seconds == 0 { t("serversettings.shared.off") } else { crate::ui::moderate::duration(i64::from(seconds)) }
}

fn slow_short(seconds: i32) -> String {
    match seconds {
        s if s >= 3600 => t_with("desktop.server.channels.hoursShort", &[("count", Arg::Num((s / 3600).into()))]),
        s if s >= 60 => t_with("desktop.server.channels.minutesShort", &[("count", Arg::Num((s / 60).into()))]),
        s => t_with("desktop.server.channels.secondsShort", &[("count", Arg::Num(s.into()))]),
    }
}

fn slow_index(seconds: i32) -> usize {
    SLOW.iter().position(|s| *s >= seconds).unwrap_or(SLOW.len() - 1)
}

fn kind(c: &pb::Channel) -> pb::ChannelType {
    pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text)
}

fn glyph(c: &pb::Channel) -> &'static str {
    match kind(c) {
        pb::ChannelType::Category => "folder",
        pb::ChannelType::Voice => "volume-2",
        pb::ChannelType::Announcement => "megaphone",
        pb::ChannelType::Secure => "shield-check",
        _ => "hash",
    }
}

/// Text-like channels have a topic and slow mode.
fn texty(c: &pb::Channel) -> bool {
    matches!(kind(c), pb::ChannelType::Text | pb::ChannelType::Announcement | pb::ChannelType::Secure)
}

/// `text-emerald-700 dark:text-emerald-300`, the color of a channel in step with its category.
fn emerald(p: &Palette) -> Hsla {
    gpui_kit::rgb(if p.dark { 0x6ee7b7 } else { 0x047857 }).into()
}

pub(super) struct Channels {
    selected: Option<String>,
    tab: Tab,
    name: Entity<InputState>,
    topic: Entity<TextareaState>,
    slow: Entity<SliderState>,
    /// What the boxes were filled with, to tell typing from a change made elsewhere.
    name_base: String,
    topic_base: String,
    filled_for: Option<String>,
    parent: Option<String>,
    slowmode: Option<i32>,
    saving: bool,
    confirming: bool,
    /// A channel moving, until the server answers.
    moving: bool,
    /// Permissions not saved yet; `None` follows the channel as it is.
    draft: Option<Vec<Over>>,
    target: Option<String>,
    perm_saving: bool,
    /// Whether the first loose channel was picked when the page opened.
    started: bool,
    /// What went wrong with the last save, said in the save bar.
    error: Option<String>,
}

impl Channels {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.server.channels.namePlaceholder")));
        let topic = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 6).placeholder(t("desktop.server.channels.topicPlaceholder"))
        });
        let slow = cx.new(|_| SliderState::new().min(0.0).max((SLOW.len() - 1) as f32).step(1.0).default_value(0.0));
        let subscriptions = vec![
            cx.subscribe(&name, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
            cx.subscribe(&topic, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
            cx.subscribe(&slow, |this: &mut ServerSettingsView, _, e: &SliderEvent, cx| {
                let SliderEvent::Change(v) = e else { return };
                let at = (v.start().round() as usize).min(SLOW.len() - 1);
                this.channels.slowmode = Some(SLOW[at]);
                cx.notify();
            }),
        ];
        let channels = Self {
            selected: None,
            tab: Tab::Overview,
            name,
            topic,
            slow,
            name_base: String::new(),
            topic_base: String::new(),
            filled_for: None,
            parent: None,
            slowmode: None,
            saving: false,
            confirming: false,
            moving: false,
            draft: None,
            target: None,
            perm_saving: false,
            started: false,
            error: None,
        };
        (channels, subscriptions)
    }
}

/// The page's view of the server.
struct Snap {
    channels: Vec<pb::Channel>,
    roles: Vec<pb::Role>,
    members: Vec<pb::Member>,
    access: Access,
    me: Option<pb::User>,
    my_roles: Vec<String>,
    owner_id: String,
    system_channel: String,
}

impl ServerSettingsView {
    fn channel_snap(&self) -> Snap {
        self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            let server = i.and_then(|i| i.server(&self.server));
            Snap {
                channels: i.and_then(|i| i.channels.get(&self.server).cloned()).unwrap_or_default(),
                roles: i.and_then(|i| i.roles.get(&self.server).cloned()).unwrap_or_default(),
                members: i.and_then(|i| i.members.get(&self.server).cloned()).unwrap_or_default(),
                access: i.map(|i| i.access(&self.server)).unwrap_or_default(),
                me: i.and_then(|i| i.me.clone()),
                my_roles: i.and_then(|i| i.my_member(&self.server)).map(|m| m.role_ids.clone()).unwrap_or_default(),
                owner_id: server.map(|s| s.owner_id.clone()).unwrap_or_default(),
                system_channel: server.map(|s| s.system_channel_id.clone()).unwrap_or_default(),
            }
        })
    }

    /// Opens on a channel's (or category's) settings, from its right-click
    /// menu; `permissions` goes straight to who can see and use it.
    pub fn edit_channel(&mut self, id: String, permissions: bool, cx: &mut Context<Self>) {
        self.open(Page::Channels, cx);
        self.pick_channel(id, cx);
        self.channels.tab = if permissions { Tab::Permissions } else { Tab::Overview };
        cx.notify();
    }

    fn pick_channel(&mut self, id: String, cx: &mut Context<Self>) {
        let c = &mut self.channels;
        if c.selected.as_deref() != Some(id.as_str()) {
            c.selected = Some(id);
            c.parent = None;
            c.slowmode = None;
            c.confirming = false;
            c.draft = None;
            c.target = None;
            self.error = None;
        }
        cx.notify();
    }

    /// Fills the boxes for the chosen channel, and follows a change made
    /// elsewhere in a box nobody's typing in.
    fn fill_channel(&mut self, channel: &pb::Channel, window: &mut Window, cx: &mut Context<Self>) {
        let c = &mut self.channels;
        let fresh = c.filled_for.as_deref() != Some(channel.id.as_str());
        let typed = c.name.read(cx).value().to_string();
        if fresh || (typed == c.name_base && channel.name != c.name_base) {
            c.name_base = channel.name.clone();
            let name = channel.name.clone();
            c.name.update(cx, |s, cx| s.set_value(name, window, cx));
        }
        let typed = c.topic.read(cx).value().to_string();
        if fresh || (typed == c.topic_base && channel.topic != c.topic_base) {
            c.topic_base = channel.topic.clone();
            let topic = channel.topic.clone();
            c.topic.update(cx, |s, cx| s.set_value(topic, window, cx));
        }
        if fresh || c.slowmode.is_none() {
            let at = slow_index(channel.slowmode_seconds) as f32;
            c.slow.update(cx, |s, cx| s.set_value(at, window, cx));
        }
        c.filled_for = Some(channel.id.clone());
    }

    /// The name as it'll be saved: categories and voice channels keep it as typed.
    fn typed_name(&self, channel: &pb::Channel, cx: &Context<Self>) -> String {
        let typed = self.channels.name.read(cx).value().to_string();
        match kind(channel) {
            pb::ChannelType::Category | pb::ChannelType::Voice => typed.trim().to_owned(),
            _ => slug(&typed),
        }
    }

    fn channel_patch(&self, channel: &pb::Channel, cx: &Context<Self>) -> ChannelPatch {
        let c = &self.channels;
        let name = self.typed_name(channel, cx);
        let topic = c.topic.read(cx).value().trim().to_owned();
        ChannelPatch {
            name: (c.name.read(cx).value().as_ref() as &str != channel.name && name != channel.name).then_some(name),
            topic: (texty(channel) && topic != channel.topic.trim()).then_some(topic),
            parent_id: c.parent.clone().filter(|p| *p != channel.parent_id),
            slowmode_seconds: c.slowmode.filter(|s| *s != channel.slowmode_seconds),
        }
    }

    fn save_channel(&mut self, channel: &pb::Channel, cx: &mut Context<Self>) {
        let patch = self.channel_patch(channel, cx);
        if patch.name.as_deref() == Some("") {
            self.channels.error = Some(t("serversettings.channels.needsName"));
            cx.notify();
            return;
        }
        self.channels.saving = true;
        self.channels.error = None;
        let (core, key, sid, cid) = (self.core.clone(), self.key.clone(), self.server.clone(), channel.id.clone());
        self.run(cx, async move { core.update_channel(&key, &sid, &cid, patch).await }, |this, result, cx| {
            this.channels.saving = false;
            match result {
                Ok(_) => {
                    this.channels.parent = None;
                    this.channels.slowmode = None;
                    // Refill from what the server kept.
                    this.channels.filled_for = None;
                    this.flash_saved(cx);
                }
                Err(err) => this.channels.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn discard_channel(&mut self, cx: &mut Context<Self>) {
        let c = &mut self.channels;
        c.parent = None;
        c.slowmode = None;
        c.filled_for = None;
        c.error = None;
        cx.notify();
    }

    fn delete_channel(&mut self, id: String, cx: &mut Context<Self>) {
        self.channels.saving = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.delete_channel(&key, &sid, &id).await }, |this, result, cx| {
            this.channels.saving = false;
            this.channels.confirming = false;
            match result {
                Ok(()) => this.channels.selected = None,
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn save_permissions(&mut self, channel_id: String, cx: &mut Context<Self>) {
        let Some(draft) = self.channels.draft.clone() else { return };
        self.channels.perm_saving = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let wire = to_wire(&draft);
        self.run(
            cx,
            async move { core.set_channel_permissions(&key, &sid, &channel_id, wire).await },
            |this, result, cx| {
                this.channels.perm_saving = false;
                match result {
                    Ok(_) => {
                        this.channels.draft = None;
                        this.flash_saved(cx);
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// Changes one role's or person's rules in the draft, adding them if they're new.
    fn put_over(&mut self, base: &[Over], target_id: &str, member: bool, f: impl FnOnce(&mut Over)) {
        let draft = self.channels.draft.get_or_insert_with(|| base.to_vec());
        if !draft.iter().any(|o| o.target_id == target_id) {
            draft.push(Over { target_id: target_id.into(), member, allow: 0, deny: 0 });
        }
        if let Some(o) = draft.iter_mut().find(|o| o.target_id == target_id) {
            f(o);
        }
    }

    pub(super) fn channels_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.channel_snap();
        let layout = arrange::layout_of(&snap.channels);
        let by_id: HashMap<&str, &pb::Channel> = snap.channels.iter().map(|c| (c.id.as_str(), c)).collect();
        // Channels you can't see aren't yours to change.
        let visible = |id: &str| snap.access.channels.contains_key(id);
        let can_arrange = snap.access.has(P::ManageChannels);

        // As on the web: the first loose channel to begin with, and nothing once the chosen one's gone.
        if !self.channels.started {
            self.channels.started = true;
            if self.channels.selected.is_none()
                && let Some(first) = layout.loose.iter().find(|id| visible(id))
            {
                self.pick_channel(first.clone(), cx);
            }
        }
        let selected = self.channels.selected.clone().filter(|id| by_id.contains_key(id.as_str()));

        // The list: every row at its place, sliding when the order changes.
        let mut entries: Vec<(String, bool, f32)> = Vec::new();
        let mut y = 0.0;
        for id in layout.loose.iter().filter(|id| visible(id)) {
            entries.push((id.clone(), false, y));
            y += ROW;
        }
        for (cat, children) in &layout.categories {
            if !visible(cat) {
                continue;
            }
            y += CATEGORY_GAP;
            entries.push((cat.clone(), true, y));
            y += ROW;
            for id in children.iter().filter(|id| visible(id)) {
                entries.push((id.clone(), false, y));
                y += ROW;
            }
        }
        let mut rows = div().relative().h(px((y - 2.0).max(0.0)));
        // The highlight glides between rows.
        if let Some((id, category, top)) = entries.iter().find(|(id, _, _)| selected.as_deref() == Some(id.as_str())) {
            let nested = !category && by_id.get(id.as_str()).is_some_and(|c| self.is_nested(c, &layout));
            let left = if can_arrange { 30.0 } else { 0.0 } + if nested { 12.0 } else { 0.0 };
            let at = motion::follow("chan-hl-y", *top, window, cx);
            let x = motion::follow("chan-hl-x", left, window, cx);
            let right = if *category && snap.access.has_in(id, P::ManageChannels) { 30.0 } else { 0.0 };
            rows = rows.child(
                div()
                    .absolute()
                    .left(px(x))
                    .right(px(right))
                    .top(px(at))
                    .h(px(32.0))
                    .rounded(radius_lg())
                    .bg(alpha(p.primary, 0.12)),
            );
        }
        for (n, (id, category, top)) in entries.iter().enumerate() {
            let Some(c) = by_id.get(id.as_str()) else { continue };
            let at = motion::follow(SharedString::from(format!("chan-y-{id}")), *top, window, cx);
            let row = self.channel_row(
                c,
                *category,
                selected.as_deref() == Some(id.as_str()),
                can_arrange,
                &layout,
                &snap,
                p,
                cx,
            );
            rows = rows.child(div().absolute().left_0().right_0().top(px(at)).child(motion::rise(
                row,
                SharedString::from(format!("chan-in-{id}")),
                Duration::from_millis(20 * n.min(14) as u64),
                6.0,
            )));
        }
        let list = div()
            .w(px(304.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(div().min_w_0().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(
                        if can_arrange {
                            t("serversettings.channels.introArrange")
                        } else {
                            t("serversettings.channels.introPick")
                        },
                    ))
                    .when(can_arrange, |el| {
                        el.child(
                            button("channel-new", t("serversettings.shared.new"), Some("plus"), Look::Primary, true, p)
                                .rounded(radius_xl())
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(ServerSettingsEvent::CreateChannel { parent: String::new() })
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .p(px(8.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.4))
                    .child(rows),
            );

        let editor = match selected.as_deref().and_then(|id| by_id.get(id)).map(|c| (*c).clone()) {
            Some(channel) => motion::slide_in(
                self.channel_editor(&channel, &snap, p, window, cx),
                SharedString::from(format!("chan-editor-{}", channel.id)),
                16.0,
            )
            .into_any_element(),
            None => div()
                .py(px(40.0))
                .text_center()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t("serversettings.channels.pick"))
                .into_any_element(),
        };
        div()
            .flex()
            .items_start()
            .gap(px(32.0))
            .child(list)
            .child(div().flex_1().min_w_0().child(editor))
            .into_any_element()
    }

    /// Where something dragged onto a row lands: before a channel (after it when coming from
    /// above), into a category at its top, or a category before another's.
    fn drop_channel(
        &mut self,
        layout: &Layout,
        dragged: &ChanDrag,
        onto: &str,
        onto_category: bool,
        cx: &mut Context<Self>,
    ) {
        if dragged.id == onto || self.channels.moving {
            return;
        }
        let order: Vec<String> = arrange::placements(layout).into_iter().map(|p| p.channel_id).collect();
        let below = order.iter().position(|x| *x == dragged.id) < order.iter().position(|x| x == onto);
        let drop = if dragged.category {
            let ids: Vec<&String> = layout.categories.iter().map(|(c, _)| c).collect();
            let target = if onto_category {
                onto.to_owned()
            } else {
                match layout.categories.iter().find(|(_, ch)| ch.iter().any(|c| c == onto)) {
                    Some((c, _)) => c.clone(),
                    None => return,
                }
            };
            let at = ids.iter().position(|c| **c == target).unwrap_or(0);
            let before = if below { ids.get(at + 1).map(|c| (*c).clone()) } else { Some(target) };
            arrange::Drop::Category { id: dragged.id.clone(), before }
        } else if onto_category {
            let first = layout.categories.iter().find(|(c, _)| c == onto).and_then(|(_, ch)| ch.first().cloned());
            arrange::Drop::Channel { id: dragged.id.clone(), parent: onto.to_owned(), before: first }
        } else {
            let (parent, list): (String, &[String]) =
                match layout.categories.iter().find(|(_, ch)| ch.iter().any(|c| c == onto)) {
                    Some((c, ch)) => (c.clone(), ch.as_slice()),
                    None => (String::new(), layout.loose.as_slice()),
                };
            let at = list.iter().position(|c| c == onto).unwrap_or(0);
            let before = if below { list.get(at + 1).cloned() } else { Some(onto.to_owned()) };
            arrange::Drop::Channel { id: dragged.id.clone(), parent, before }
        };
        let next = arrange::moved(layout, &drop);
        if next == *layout {
            return;
        }
        self.channels.moving = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.reorder_channels(&key, &sid, next).await }, |this, result, cx| {
            this.channels.moving = false;
            if let Err(err) = result {
                cx.emit(ServerSettingsEvent::Toast { icon: "circle-alert", title: err.message });
            }
            cx.notify();
        });
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn channel_row(
        &self,
        c: &pb::Channel,
        category: bool,
        active: bool,
        can_arrange: bool,
        layout: &Layout,
        snap: &Snap,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui_kit::Div> {
        let (hover, fg) = (alpha(p.muted, 0.7), p.foreground);
        let handle = can_arrange.then(|| {
            let drag = ChanDrag { id: c.id.clone(), category, name: c.name.clone().into(), glyph: glyph(c) };
            let (bg, fg) = (p.muted, p.foreground);
            div()
                .id(SharedString::from(format!("chan-grip-{}", c.id)))
                .size(px(28.0))
                .flex_none()
                .rounded(radius_md())
                .flex()
                .items_center()
                .justify_center()
                .cursor_grab()
                .text_color(alpha(p.muted_foreground, 0.6))
                .hover(move |s| s.bg(bg).text_color(fg))
                .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                .child(icon("grip-vertical").size(px(16.0)))
        });
        let private = permissions::is_private(c, &self.server);
        let id = c.id.clone();
        let mut pick = div()
            .id(SharedString::from(format!("chan-pick-{}", c.id)))
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(radius_lg())
            .cursor_pointer()
            .when(active, |el| el.text_color(p.primary).font_weight(FontWeight::BOLD))
            .when(!active, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover).text_color(fg)))
            .on_click(cx.listener(move |this, _, _, cx| this.pick_channel(id.clone(), cx)))
            .child(icon(glyph(c)).size(px(if category { 14.0 } else { 16.0 })))
            .child(div().min_w_0().truncate().map(|el| {
                if category {
                    el.text_xs().font_weight(FontWeight::BOLD).child(c.name.to_uppercase())
                } else {
                    el.text_sm().child(c.name.clone())
                }
            }));
        if private {
            pick = pick.child(motion::once(
                div().text_color(p.muted_foreground).child(icon("lock").size(px(12.0))),
                SharedString::from(format!("chan-lock-{}", c.id)),
                Duration::from_millis(380),
                |el, t| el.opacity(t),
            ));
        }
        if !category && c.slowmode_seconds > 0 {
            pick = pick.child(div().flex_1()).child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .text_size(px(11.2))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(p.muted_foreground)
                    .child(icon("snail").size(px(12.0)))
                    .child(slow_short(c.slowmode_seconds)),
            );
        }
        let add = (category && snap.access.has_in(&c.id, P::ManageChannels)).then(|| {
            let parent = c.id.clone();
            let (bg, fg) = (p.muted, p.foreground);
            div()
                .id(SharedString::from(format!("chan-add-{}", c.id)))
                .size(px(28.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius_md())
                .cursor_pointer()
                .text_color(p.muted_foreground)
                .hover(move |s| s.bg(bg).text_color(fg))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ServerSettingsEvent::CreateChannel { parent: parent.clone() })
                }))
                .child(icon("plus").size(px(14.0)))
        });
        let (onto, layout) = (c.id.clone(), layout.clone());
        let drop_hl = alpha(p.primary, 0.08);
        div()
            .id(SharedString::from(format!("chan-row-{}", c.id)))
            .h(px(32.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .rounded(radius_lg())
            .when(!category, |el| el.pl(px(if self.is_nested(c, &layout) { 12.0 } else { 0.0 })))
            .drag_over::<ChanDrag>(move |s, _, _, _| s.bg(drop_hl))
            .on_drop::<ChanDrag>(
                cx.listener(move |this, drag: &ChanDrag, _, cx| this.drop_channel(&layout, drag, &onto, category, cx)),
            )
            .children(handle)
            .child(pick)
            .children(add)
    }

    fn is_nested(&self, c: &pb::Channel, layout: &Layout) -> bool {
        layout.categories.iter().any(|(_, children)| children.contains(&c.id))
    }

    fn channel_editor(
        &mut self,
        channel: &pb::Channel,
        snap: &Snap,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let manage = snap.access.has_in(&channel.id, P::ManageChannels);
        let roles = snap.access.has_in(&channel.id, P::ManageRoles);
        let mut options = Vec::new();
        if manage {
            options.push((Tab::Overview, t("serversettings.nav.overview")));
        }
        if roles {
            options.push((Tab::Permissions, t("serversettings.shared.permissions")));
        }
        // Sharing takes managing the server and the channel, while the instance allows it (or it's shared already).
        let texty = matches!(kind(channel), pb::ChannelType::Text | pb::ChannelType::Announcement);
        let sharing_on = self
            .core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.node.as_ref()).is_some_and(|n| n.shared_channels));
        if texty && snap.access.has(P::ManageServer) && manage && (sharing_on || channel.shared.is_some()) {
            options.push((Tab::Share, t("serversettings.channels.share")));
        }
        let tab =
            options.iter().map(|(t, _)| *t).find(|t| *t == self.channels.tab).or(options.first().map(|(t, _)| *t));
        let chosen = options.iter().position(|(t, _)| Some(*t) == tab).unwrap_or(0);
        let tabs_of: Vec<Tab> = options.iter().map(|(t, _)| *t).collect();
        let labels: Vec<String> = options.into_iter().map(|(_, l)| l).collect();
        let tabs = (labels.len() > 1).then(|| {
            self.seg_tabs("chan-tabs", labels, chosen, p, window, cx, move |this, n, cx| {
                this.channels.tab = tabs_of[n];
                cx.notify();
            })
        });
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.0))
            .mb(px(20.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_lg()
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(icon(glyph(channel)).size(px(20.0)).text_color(p.muted_foreground))
                    .child(div().truncate().child(channel.name.clone())),
            )
            .children(tabs);
        let body = match tab {
            Some(Tab::Overview) => self.channel_overview(channel, snap, p, window, cx).into_any_element(),
            Some(Tab::Permissions) => self.channel_permissions(channel, snap, p, window, cx).into_any_element(),
            Some(Tab::Share) => self.channel_share(channel, p, window, cx),
            None => div()
                .py(px(40.0))
                .text_center()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t("serversettings.channels.cantChange"))
                .into_any_element(),
        };
        div().flex().flex_col().child(header).child(motion::rise(
            div().child(body),
            SharedString::from(format!("chan-body-{}-{}", channel.id, tab.map(|t| t as u8).unwrap_or(9))),
            Duration::ZERO,
            10.0,
        ))
    }

    fn channel_overview(
        &mut self,
        channel: &pb::Channel,
        snap: &Snap,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        self.fill_channel(channel, window, cx);
        let category = kind(channel) == pb::ChannelType::Category;
        let typed = self.channels.name.read(cx).value().to_string();
        let will_be = self.typed_name(channel, cx);
        let mut out = div().flex().flex_col();

        let name_box = div()
            .relative()
            .child(
                super::pages::boxed(
                    Input::new(&self.channels.name).appearance(false),
                    44.0,
                    super::pages::focused(&self.channels.name, window, cx),
                    p,
                )
                .when(!category, |el| el.pl(px(28.0))),
            )
            .when(!category, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(px(12.0))
                        .top(px(14.0))
                        .text_color(p.muted_foreground)
                        .child(icon("hash").size(px(16.0))),
                )
            });
        let renamed = (!will_be.is_empty() && will_be != typed.trim())
            .then(|| t_with("desktop.server.channels.savedAs", &[("name", Arg::Str(&will_be))]));
        out = out.child(
            form_row(
                &if category {
                    t("serversettings.channels.categoryName")
                } else {
                    t("serversettings.channels.channelName")
                },
                renamed,
                name_box,
                false,
                p,
            )
            // The first row sits right under the tabs, as the web's does.
            .pt(px(0.0)),
        );

        if texty(channel) {
            out = out.child(form_row(
                &t("serversettings.channels.topic"),
                Some(t("serversettings.channels.topicHint")),
                crate::ui::instance_home::focus_ring(
                    div()
                        .w_full()
                        .min_h(px(62.0))
                        // The box keeps its own padding, so this brings the words to the web's 12 and 8.
                        .px(px(2.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .text_sm()
                        .child(Textarea::new(&self.channels.topic).appearance(false)),
                    super::pages::focused(&self.channels.topic, window, cx),
                    p,
                ),
                false,
                p,
            ));
        }

        if !category {
            let parent = self.channels.parent.clone().unwrap_or_else(|| channel.parent_id.clone());
            let cats: Vec<(String, String)> = snap
                .channels
                .iter()
                .filter(|c| kind(c) == pb::ChannelType::Category)
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect();
            let parent_name = cats
                .iter()
                .find(|(id, _)| *id == parent)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t("serversettings.channels.noCategory"));
            let open = self.menu_open("chan-parent");
            let hover = alpha(p.primary, 0.4);
            let trigger = div()
                .id("chan-parent")
                .w_full()
                .h(px(44.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_xl())
                .border_1()
                .px(px(12.0))
                .text_sm()
                .cursor_pointer()
                .map(|el| {
                    if open {
                        el.border_color(alpha(p.primary, 0.6))
                    } else {
                        el.border_color(p.border).hover(move |s| s.border_color(hover))
                    }
                })
                .child(icon("folder").size(px(16.0)).text_color(p.muted_foreground))
                .child(div().flex_1().truncate().font_weight(FontWeight::BOLD).child(parent_name));
            let mut items = vec![
                Item::action(t("serversettings.channels.noCategory"), None, |this, _, cx| {
                    this.channels.parent = Some(String::new());
                    cx.notify();
                })
                .radio(parent.is_empty()),
            ];
            for (id, name) in cats {
                let on = id == parent;
                items.push(
                    Item::action(name, None, move |this, _, cx| {
                        this.channels.parent = Some(id.clone());
                        cx.notify();
                    })
                    .radio(on),
                );
            }
            let menu = self.dropdown("chan-parent".into(), trigger, items, false, 256.0, 48.0, p, cx);
            out =
                out.child(form_row(&t("serversettings.channels.category"), None, div().w_full().child(menu), false, p));
        }

        if texty(channel) {
            let seconds = self.channels.slowmode.unwrap_or(channel.slowmode_seconds);
            let on = seconds > 0;
            // The snail creeps while slow mode is on, faster the slower it is.
            let speed = 2400 - 120 * slow_index(seconds).min(12) as u64;
            let snail = div()
                .size(px(40.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius_xl())
                .bg(if on { alpha(p.primary, 0.15) } else { p.muted.into() })
                .text_color(if on { p.primary } else { p.muted_foreground })
                .child(if on {
                    motion::ambient(
                        div().child(icon("snail").size(px(20.0))),
                        SharedString::from(format!("chan-snail-{speed}")),
                        Duration::from_millis(speed),
                        window,
                        |el, t| el.relative().left(px(3.0 * (t * std::f32::consts::PI).sin())),
                    )
                } else {
                    icon("snail").size(px(20.0)).into_any_element()
                });
            let row = form_row(
                &t("serversettings.nav.slowmode"),
                Some(t("serversettings.channels.slowmodeHint")),
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(snail)
                    .child(div().flex_1().min_w_0().px(px(8.0)).child(self.slow_slider(seconds, p, cx)))
                    .child(
                        div()
                            .min_w(px(80.0))
                            .flex_none()
                            .text_right()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .whitespace_nowrap()
                            .child(slow_label(seconds)),
                    ),
                false,
                p,
            );
            out = out.child(self.mark("slowmode", row, p));
        }

        // Deleting, behind a confirmation.
        let id = channel.id.clone();
        let saving = self.channels.saving;
        let delete = if self.channels.confirming {
            let system = snap.system_channel == channel.id;
            motion::rise(
                div()
                    .w_full()
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
                            .child(if category {
                                t_with(
                                    "serversettings.channels.deleteCategoryAsk",
                                    &[("name", Arg::Str(&channel.name))],
                                )
                            } else {
                                t_with("serversettings.channels.deleteChannelAsk", &[("name", Arg::Str(&channel.name))])
                            }),
                    )
                    .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(if category {
                        t("serversettings.channels.deleteCategoryHint")
                    } else if system {
                        t("serversettings.channels.deleteSystemHint")
                    } else {
                        t("serversettings.channels.deleteChannelHint")
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                button("chan-keep", t("serversettings.shared.keepIt"), None, Look::Ghost, false, p)
                                    .rounded(radius_xl())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.channels.confirming = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button(
                                    "chan-delete-yes",
                                    t("serversettings.shared.delete"),
                                    if saving { None } else { Some("trash") },
                                    Look::Destructive,
                                    false,
                                    p,
                                )
                                .rounded(radius_xl())
                                .font_weight(FontWeight::BOLD)
                                .when(saving, |el| el.opacity(0.6))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.channels.saving {
                                        this.delete_channel(id.clone(), cx)
                                    }
                                })),
                            ),
                    ),
                "chan-confirm",
                Duration::ZERO,
                8.0,
            )
            .into_any_element()
        } else {
            let red = alpha(p.destructive, 0.1);
            super::pages::hover_button(
                "chan-delete",
                if category {
                    t("serversettings.channels.deleteCategory")
                } else {
                    t("serversettings.channels.deleteChannel")
                },
                Some("trash"),
                false,
                p,
                move |s| s.bg(red),
            )
            .text_color(p.destructive)
            .font_weight(FontWeight::MEDIUM)
            .on_click(cx.listener(|this, _, _, cx| {
                this.channels.confirming = true;
                cx.notify();
            }))
            .into_any_element()
        };
        out = out.child(div().py(px(20.0)).flex().child(delete));

        let patch = self.channel_patch(channel, cx);
        let n =
            [patch.name.is_some(), patch.topic.is_some(), patch.parent_id.is_some(), patch.slowmode_seconds.is_some()]
                .into_iter()
                .filter(|c| *c)
                .count();
        if n > 0 {
            let c = channel.clone();
            self.bar = Some(bar_with_error(
                "chan-save-bar",
                n,
                self.channels.saving,
                self.channels.error.as_deref(),
                p,
                cx,
                |this, _, cx| this.discard_channel(cx),
                move |this, _, cx| {
                    if !this.channels.saving {
                        this.save_channel(&c, cx)
                    }
                },
            ));
        }
        out
    }

    fn channel_permissions(
        &mut self,
        channel: &pb::Channel,
        snap: &Snap,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let everyone_id = self.server.clone();
        let category = kind(channel) == pb::ChannelType::Category;
        let base = from_wire(&channel.permission_overwrites);
        let draft = self.channels.draft.clone().unwrap_or_else(|| base.clone());
        let parent =
            snap.channels.iter().find(|c| c.id == channel.parent_id && kind(c) == pb::ChannelType::Category).cloned();
        let find = |id: &str| draft.iter().find(|o| o.target_id == id).cloned();
        let everyone =
            find(&everyone_id).unwrap_or(Over { target_id: everyone_id.clone(), member: false, allow: 0, deny: 0 });
        let private = everyone.deny & VIEW != 0;
        let have = snap.access.channels.get(&channel.id).copied().unwrap_or(0);
        let may = |perm: P| snap.access.may_change(bit(perm), have);
        let role_of = |id: &str| snap.roles.iter().find(|r| r.id == id);
        let member_of = |id: &str| snap.members.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == id));

        // Roles first, highest first, then people.
        let order: HashMap<&str, usize> = snap.roles.iter().enumerate().map(|(n, r)| (r.id.as_str(), n)).collect();
        let mut targets: Vec<Over> = draft.iter().filter(|o| o.target_id != everyone_id).cloned().collect();
        targets.sort_by_key(|o| (o.member, order.get(o.target_id.as_str()).copied().unwrap_or(usize::MAX)));
        let viewers: Vec<Over> = targets.iter().filter(|o| o.allow & VIEW != 0).cloned().collect();

        // What saving would do to you: losing the channel is worth a warning.
        let losing = snap.me.as_ref().is_some_and(|me| {
            let after = pb::Channel { permission_overwrites: to_wire(&draft), ..channel.clone() };
            let list: Vec<pb::Channel> =
                snap.channels.iter().map(|c| if c.id == channel.id { after.clone() } else { c.clone() }).collect();
            let you =
                permissions::access_of(&everyone_id, &snap.owner_id, &snap.roles, &list, &me.id, &snap.my_roles, false);
            !you.channels.contains_key(&channel.id)
        });

        let mut out = div().flex().flex_col().gap(px(20.0));

        // Same as its category, or its own rules.
        if let Some(parent) = &parent {
            let synced = canon(&from_wire(&parent.permission_overwrites)) == canon(&draft);
            let green = emerald(p);
            let pb_over = from_wire(&parent.permission_overwrites);
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .text_sm()
                    .bg(if synced { Hsla::from(gpui_kit::rgb(0x10b981)).opacity(0.1) } else { alpha(p.muted, 0.6) })
                    .when(synced, |el| el.text_color(green))
                    .child(motion::once(
                        icon(if synced { "link" } else { "unlink" }).size(px(16.0)),
                        SharedString::from(format!("chan-sync-{synced}")),
                        Duration::from_millis(320),
                        |el, t| el.opacity(t),
                    ))
                    .child(div().flex_1().min_w_0().line_height(px(20.0)).child(marked(
                        &t_with(
                            if synced {
                                "serversettings.channelPermissions.synced"
                            } else {
                                "serversettings.channelPermissions.apart"
                            },
                            &[("category", Arg::Str(&strong(&parent.name)))],
                        ),
                        p,
                    )))
                    .when(!synced, |el| {
                        el.child(
                            button(
                                "chan-sync",
                                t("serversettings.channelPermissions.match"),
                                None,
                                Look::Ghost,
                                true,
                                p,
                            )
                            .h(px(28.0))
                            .rounded(radius_lg())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.channels.draft = Some(pb_over.clone());
                                cx.notify();
                            })),
                        )
                    }),
            );
        }

        // The private switch, and who can see it.
        let lock_tile = div()
            .size(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius_xl())
            .bg(if private { alpha(p.primary, 0.15) } else { p.muted.into() })
            .text_color(if private { p.primary } else { p.muted_foreground })
            .child(motion::once(
                icon(if private { "lock" } else { "lock-open" }).size(px(20.0)),
                SharedString::from(format!("chan-lock-tile-{private}")),
                Duration::from_millis(420),
                |el, t| el.mt(px((1.0 - t) * 10.0)).opacity(t),
            ));
        let base_for_switch = base.clone();
        let eid = everyone_id.clone();
        let mut private_card = div()
            .flex()
            .flex_col()
            .p(px(16.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(lock_tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(if category {
                                t("serversettings.channelPermissions.privateCategory")
                            } else {
                                t("serversettings.channelPermissions.privateChannel")
                            }))
                            .child(div().text_sm().text_color(p.muted_foreground).child(if category {
                                t("serversettings.channelPermissions.privateCategoryHint")
                            } else {
                                t("serversettings.channelPermissions.privateChannelHint")
                            })),
                    )
                    .child(crate::ui::settings_controls::switch(
                        "chan-private",
                        private,
                        !may(P::ViewChannels),
                        p,
                        window,
                        cx,
                        move |this: &mut Self, on, cx| {
                            this.put_over(&base_for_switch, &eid, false, |o| {
                                if on {
                                    o.allow &= !VIEW;
                                    o.deny |= VIEW;
                                } else {
                                    o.deny &= !VIEW;
                                }
                            });
                            cx.notify();
                        },
                    )),
            );
        if private {
            let mut chips = div().flex().flex_wrap().gap(px(6.0));
            for (n, o) in viewers.iter().enumerate() {
                let (id, member) = (o.target_id.clone(), o.member);
                let base = base.clone();
                let hover = alpha(p.destructive, 0.12);
                let fg = p.destructive;
                chips = chips.child(motion::rise(
                    div()
                        .h(px(30.0))
                        .pl(px(8.0))
                        .pr(px(3.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .rounded_full()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.card)
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .child(self.target_badge(o, role_of(&o.target_id), member_of(&o.target_id), p))
                        .child(
                            div()
                                .id(SharedString::from(format!("chan-viewer-x-{id}")))
                                .size(px(22.0))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted_foreground)
                                .when(may(P::ViewChannels), |el| {
                                    el.cursor_pointer().hover(move |s| s.bg(hover).text_color(fg)).on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.put_over(&base, &id, member, |o| o.allow &= !VIEW);
                                            cx.notify();
                                        }),
                                    )
                                })
                                .child(icon("x").size(px(12.0))),
                        ),
                    SharedString::from(format!("chan-viewer-{}", o.target_id)),
                    Duration::from_millis(30 * n.min(8) as u64),
                    -4.0,
                ));
            }
            let viewer_roles: Vec<pb::Role> = snap
                .roles
                .iter()
                .filter(|r| r.id != everyone_id && find(&r.id).is_none_or(|o| o.allow & VIEW == 0))
                .cloned()
                .collect();
            let viewer_people: Vec<pb::Member> = snap
                .members
                .iter()
                .filter(|m| m.user.as_ref().is_some_and(|u| find(&u.id).is_none_or(|o| o.allow & VIEW == 0)))
                .cloned()
                .collect();
            if may(P::ViewChannels) && (!viewer_roles.is_empty() || !viewer_people.is_empty()) {
                let open = self.menu_open("chan-viewer-add");
                let (hover, fg) = (alpha(p.primary, 0.5), p.primary);
                let trigger = div()
                    .id("chan-viewer-add")
                    .size(px(32.0))
                    .rounded_full()
                    .border_1()
                    .border_dashed()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .map(|el| {
                        if open {
                            el.border_color(p.border).text_color(p.primary)
                        } else {
                            el.border_color(p.border)
                                .text_color(p.muted_foreground)
                                .hover(move |s| s.border_color(hover).text_color(fg))
                        }
                    })
                    .child(icon("plus").size(px(16.0)));
                let items = self.add_items(&viewer_roles, &viewer_people, true, &base, p);
                chips = chips.child(self.dropdown("chan-viewer-add".into(), trigger, items, true, 240.0, 36.0, p, cx));
            }
            let mut who = div()
                .mt(px(14.0))
                .pt(px(14.0))
                .border_t_1()
                .border_color(p.border)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.muted_foreground)
                        .child(t("serversettings.channelPermissions.whoCanSee").to_uppercase()),
                )
                .child(chips);
            if viewers.is_empty() {
                who = who.child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("serversettings.channelPermissions.nobodyYet")),
                );
            }
            private_card = private_card.child(motion::rise(who, "chan-who", Duration::ZERO, -6.0));
        }
        out = out.child(private_card);

        if losing {
            let warn = amber(p);
            out = out.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(warn.opacity(0.4))
                    .bg(warn.opacity(0.1))
                    .text_sm()
                    .text_color(warn)
                    .child(icon("triangle-alert").size(px(16.0)))
                    .child(div().flex_1().min_w_0().child(if category {
                        t("serversettings.channelPermissions.losingCategory")
                    } else {
                        t("serversettings.channelPermissions.losingChannel")
                    })),
                "chan-losing",
                Duration::ZERO,
                -6.0,
            ));
        }

        // Advanced: every channel permission, per role or person.
        let target_roles: Vec<pb::Role> =
            snap.roles.iter().filter(|r| r.id != everyone_id && find(&r.id).is_none()).cloned().collect();
        let target_people: Vec<pb::Member> =
            snap.members.iter().filter(|m| m.user.as_ref().is_some_and(|u| find(&u.id).is_none())).cloned().collect();
        let add_target = (!target_roles.is_empty() || !target_people.is_empty()).then(|| {
            let trigger = button(
                "chan-target-add",
                t("serversettings.channelPermissions.add"),
                Some("plus"),
                Look::Outline,
                true,
                p,
            )
            .rounded(radius_xl())
            .font_weight(FontWeight::BOLD);
            let items = self.add_items(&target_roles, &target_people, false, &base, p);
            self.dropdown("chan-target-add".into(), trigger, items, true, 240.0, 36.0, p, cx)
        });
        let mut advanced = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .line_height(px(24.0))
                                .child(t("serversettings.channelPermissions.advanced")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.channelPermissions.advancedHint")),
                        ),
                )
                .children(add_target),
        );

        let selected = self
            .channels
            .target
            .clone()
            .filter(|id| *id == everyone_id || targets.iter().any(|o| o.target_id == *id))
            .unwrap_or_else(|| everyone_id.clone());
        let mut side = div().w(px(192.0)).flex_none().flex().flex_col().gap(px(2.0));
        for o in std::iter::once(&everyone).chain(targets.iter()) {
            let on = o.target_id == selected;
            let count = permissions::to_list(o.allow).len() + permissions::to_list(o.deny).len();
            let hover = alpha(p.muted, 0.7);
            let id = o.target_id.clone();
            side = side.child(
                div()
                    .id(SharedString::from(format!("chan-target-{}", o.target_id)))
                    .h(px(36.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .cursor_pointer()
                    .text_sm()
                    .when(on, |el| el.bg(alpha(p.primary, 0.12)).font_weight(FontWeight::BOLD))
                    .when(!on, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.channels.target = Some(id.clone());
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().flex().items_center().gap(px(8.0)).map(|el| {
                        if o.target_id == everyone_id {
                            el.child(icon("users").size(px(15.0))).child(div().truncate().child("@everyone"))
                        } else {
                            el.child(self.target_badge(o, role_of(&o.target_id), member_of(&o.target_id), p))
                        }
                    }))
                    .when(count > 0, |el| {
                        el.child(
                            div()
                                .px(px(6.0))
                                .rounded_full()
                                .bg(p.muted)
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .child(count.to_string()),
                        )
                    }),
            );
        }
        let current =
            if selected == everyone_id { everyone.clone() } else { find(&selected).unwrap_or(everyone.clone()) };
        let voice = kind(channel) == pb::ChannelType::Voice;
        let mut grid = div()
            .flex_1()
            .min_w_0()
            .p(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .flex()
            .flex_col()
            .gap(px(10.0));
        let mut k = 0usize;
        for (title, list) in CHANNEL_GROUPS {
            if !(category || title == "General" || (title == "Voice") == voice) {
                continue;
            }
            let mut group = div().flex().flex_col().child(
                div()
                    .mb(px(2.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(group_name(title).to_uppercase()),
            );
            for perm in list.iter().copied() {
                let label = permission_name(perm);
                let state = if current.allow & bit(perm) != 0 {
                    1
                } else if current.deny & bit(perm) != 0 {
                    -1
                } else {
                    0
                };
                let row = div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(alpha(p.border, 0.5))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_weight(FontWeight::BOLD).child(label))
                            .child(div().text_xs().text_color(p.muted_foreground).child(permission_here(perm))),
                    )
                    .child(self.tri_state(&current, perm, state, !may(perm), &base, p, window, cx));
                group = group.child(motion::rise(
                    row,
                    SharedString::from(format!("chan-perm-{}-{}", current.target_id, perm as i32)),
                    Duration::from_millis(12 * k.min(16) as u64),
                    5.0,
                ));
                k += 1;
            }
            grid = grid.child(group);
        }
        if selected != everyone_id {
            let fg = p.destructive;
            let hover = alpha(p.destructive, 0.1);
            let id = selected.clone();
            let base = base.clone();
            grid = grid.child(
                div().flex().child(
                    div()
                        .id("chan-target-remove")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(32.0))
                        .px(px(10.0))
                        .rounded(radius_lg())
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(fg)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let draft = this.channels.draft.get_or_insert_with(|| base.clone());
                            draft.retain(|o| o.target_id != id);
                            this.channels.target = None;
                            cx.notify();
                        }))
                        .child(icon("trash").size(px(14.0)))
                        .child(if category {
                            t("serversettings.channelPermissions.removeFromCategory")
                        } else {
                            t("serversettings.channelPermissions.removeFromChannel")
                        }),
                ),
            );
        }
        advanced = advanced.child(div().flex().items_start().gap(px(12.0)).child(side).child(motion::rise(
            grid,
            SharedString::from(format!("chan-grid-{selected}")),
            Duration::ZERO,
            6.0,
        )));
        out = out.child(advanced);

        let dirty = canon(&draft) != canon(&base);
        if dirty {
            let n = changed(&base, &draft).max(1);
            let cid = channel.id.clone();
            self.bar = Some(save_bar(
                "chan-perm-bar",
                n,
                self.channels.perm_saving,
                p,
                cx,
                |this, _, cx| {
                    this.channels.draft = None;
                    this.error = None;
                    cx.notify();
                },
                move |this, _, cx| {
                    if !this.channels.perm_saving {
                        this.save_permissions(cid.clone(), cx)
                    }
                },
            ));
        }
        out
    }

    /// A role's dot and name, or a person's face and name.
    fn target_badge(
        &self,
        o: &Over,
        role: Option<&pb::Role>,
        member: Option<&pb::Member>,
        p: &Palette,
    ) -> gpui_kit::Div {
        let row = div().flex().items_center().gap(px(6.0)).min_w_0();
        if o.member {
            match member {
                Some(m) => row.child(avatar(m.user.as_ref(), 20.0, p)).child(div().truncate().child(member_name(m))),
                None => row
                    .child(icon("user").size(px(15.0)))
                    .child(div().truncate().child(t("serversettings.channelPermissions.someoneLeft"))),
            }
        } else {
            row.child(dot(role.and_then(role_color), 10.0, p)).child(div().truncate().child(
                role.map(|r| r.name.clone()).unwrap_or_else(|| t("serversettings.channelPermissions.deletedRole")),
            ))
        }
    }

    /// Roles and people to add, as chips; picking one adds them and closes it.
    #[allow(clippy::too_many_arguments)]
    /// The roles and people a menu can add (the web's `AddTarget`), letting them see it with `viewer`.
    fn add_items(
        &self,
        roles: &[pb::Role],
        people: &[pb::Member],
        viewer: bool,
        base: &[Over],
        p: &Palette,
    ) -> Vec<Item> {
        let pick = |target: String, member: bool| {
            let base = base.to_vec();
            move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                this.put_over(&base, &target, member, |o| {
                    if viewer {
                        o.allow |= VIEW;
                        o.deny &= !VIEW;
                    }
                });
                this.channels.target = Some(target.clone());
                cx.notify();
            }
        };
        let mut items = Vec::new();
        if !roles.is_empty() {
            items.push(Item::Label(t("serversettings.nav.roles")));
        }
        for r in roles {
            items.push(Item::action(
                r.name.clone(),
                Some(dot(role_color(r), 12.0, p).into_any_element()),
                pick(r.id.clone(), false),
            ));
        }
        if !roles.is_empty() && !people.is_empty() {
            items.push(Item::Separator);
        }
        if !people.is_empty() {
            items.push(Item::Label(t("serversettings.nav.people")));
        }
        for m in people.iter().take(50) {
            let Some(u) = &m.user else { continue };
            items.push(Item::action(
                member_name(m),
                Some(avatar(Some(u), 20.0, p).into_any_element()),
                pick(u.id.clone(), true),
            ));
        }
        items
    }

    /// Deny, follow their roles, or allow, the choice sliding between the three.
    #[allow(clippy::too_many_arguments)]
    fn tri_state(
        &self,
        o: &Over,
        perm: P,
        state: i32,
        disabled: bool,
        base: &[Over],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        const CELL: f32 = 28.0;
        let at = motion::follow(
            SharedString::from(format!("tri-{}-{}", o.target_id, perm as i32)),
            (state + 1) as f32 * CELL,
            window,
            cx,
        );
        let tint: Hsla = match state {
            1 => gpui_kit::rgb(0x10b981).into(),
            -1 => p.destructive.into(),
            _ => alpha(p.muted_foreground, 0.3),
        };
        let mut out = div()
            .relative()
            .flex_none()
            .flex()
            .p(px(2.0))
            .rounded(radius_lg())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .when(disabled, |el| el.opacity(0.5))
            .child(div().absolute().top(px(2.0)).left(px(2.0 + at)).size(px(CELL)).rounded(radius_md()).bg(tint));
        for (value, glyph) in [(-1, "x"), (0, "slash"), (1, "check")] {
            let on = value == state;
            let (target, member) = (o.target_id.clone(), o.member);
            let base = base.to_vec();
            let fg = p.foreground;
            out = out.child(
                div()
                    .id(SharedString::from(format!("tri-{}-{}-{value}", o.target_id, perm as i32)))
                    .relative()
                    .size(px(CELL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(if on && value != 0 {
                        gpui_kit::white()
                    } else if on {
                        fg.into()
                    } else {
                        p.muted_foreground.into()
                    })
                    .when(!disabled && !on, |el| {
                        el.cursor_pointer().hover(move |s| s.text_color(fg)).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.put_over(&base, &target, member, |o| {
                                    o.allow = if value == 1 { o.allow | bit(perm) } else { o.allow & !bit(perm) };
                                    o.deny = if value == -1 { o.deny | bit(perm) } else { o.deny & !bit(perm) };
                                });
                                cx.notify();
                            },
                        ))
                    })
                    .child(motion::once(
                        div().size(px(14.0)).child(icon(glyph).size(px(14.0))),
                        SharedString::from(format!("tri-pop-{}-{}-{value}-{on}", o.target_id, perm as i32)),
                        Duration::from_millis(if on { 260 } else { 1 }),
                        move |el, t| {
                            let s = if on { 0.6 + 0.4 * t + 0.25 * (t * std::f32::consts::PI).sin() } else { 1.0 };
                            el.scale(s)
                        },
                    )),
            );
        }
        out
    }
}

impl ServerSettingsView {
    /// Slow mode's slider, the web's `ui/slider.tsx`: a muted track filling with the primary,
    /// a ringed thumb, and the stops worth knowing under it, each a click away.
    fn slow_slider(&mut self, seconds: i32, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let state = self.channels.slow.clone();
        let at = slow_index(seconds);
        let last = (SLOW.len() - 1) as f32;
        let frac = at as f32 / last;
        let thumb = SliderThumb::new(&state)
            .absolute()
            .top(px(-6.0))
            .left(gpui_kit::relative(frac))
            .ml(px(-10.0))
            .size(px(20.0))
            .rounded_full()
            .border(px(3.0))
            .border_color(p.primary)
            .bg(p.background)
            .shadow(vec![gpui_kit::BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.1),
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(6.0),
                spread_radius: px(-1.0),
                inset: false,
            }])
            .cursor_pointer();
        let track =
            SliderTrack::new(&state).relative().w_full().h(px(20.0)).flex().items_center().cursor_pointer().child(
                SliderIndicator::new(&state)
                    .relative()
                    .w_full()
                    .h(px(8.0))
                    .rounded_full()
                    .bg(p.muted)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(gpui_kit::relative(frac))
                            .rounded_full()
                            .bg(p.primary),
                    )
                    .child(thumb),
            );
        let mut marks = div().relative().mt(px(6.0)).h(px(16.0)).text_size(px(11.2)).text_color(p.muted_foreground);
        for stop in [0usize, 4, 7, 11, 13] {
            let label = if stop == 0 { t("serversettings.shared.off") } else { slow_short(SLOW[stop]) };
            let on = stop == at;
            let fg = p.foreground;
            marks = marks.child(
                div().absolute().top_0().left(gpui_kit::relative(stop as f32 / last)).child(
                    div()
                        .id(SharedString::from(format!("slow-mark-{stop}")))
                        .relative()
                        .left(px(-30.0))
                        .w(px(60.0))
                        .flex()
                        .justify_center()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .when(on, |el| el.font_weight(FontWeight::BOLD).text_color(p.primary))
                        .when(!on, |el| el.hover(move |s| s.text_color(fg)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.channels.slowmode = Some(SLOW[stop]);
                            this.channels.slow.update(cx, |s, cx| s.set_value(stop as f32, window, cx));
                            cx.notify();
                        }))
                        .child(label),
                ),
            );
        }
        div()
            .pt(px(28.0))
            .pb(px(4.0))
            .child(BaseSlider::new(&state).relative().w_full().child(track))
            .child(marks)
            .into_any_element()
    }
}

impl ServerSettingsView {
    /// Opens the channel editor's Permissions tab (search jumping to a channel's permissions).
    pub(super) fn channels_tab_permissions(&mut self) {
        self.channels.tab = Tab::Permissions;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_slugs() {
        assert_eq!(slug("  Cozy Corner!! "), "cozy-corner");
        assert_eq!(slug("Ünïcode   ok_1"), "ünïcode-ok_1");
        assert_eq!(slug(&"a".repeat(150)).len(), 100);
    }

    #[test]
    fn slow_mode_reads_and_snaps() {
        assert_eq!(slow_label(0), "Off");
        assert_eq!(slow_label(30), "30 seconds");
        assert_eq!(slow_label(3600), "1 hour");
        assert_eq!(slow_short(21600), "6h");
        assert_eq!(slow_index(45), 5);
        assert_eq!(slow_index(99_999), SLOW.len() - 1);
    }

    #[test]
    fn drafts_count_the_people_they_change() {
        let o = |id: &str, allow: Bits, deny: Bits| Over { target_id: id.into(), member: false, allow, deny };
        let base = vec![o("everyone", 0, VIEW), o("mods", VIEW, 0)];
        assert_eq!(changed(&base, &base), 0);
        // Adding someone with nothing set changes nothing yet.
        let mut draft = base.clone();
        draft.push(o("art", 0, 0));
        assert_eq!(changed(&base, &draft), 0);
        assert_eq!(canon(&base), canon(&draft));
        draft[2].allow = VIEW;
        draft[0].deny = 0;
        assert_eq!(changed(&base, &draft), 2);
        let wire = to_wire(&draft);
        assert_eq!(wire.len(), 2);
        assert_eq!(from_wire(&wire)[1], o("art", VIEW, 0));
    }
}

/// A channel or category being dragged into place, and the copy of it that follows the pointer.
#[derive(Clone)]
pub(super) struct ChanDrag {
    id: String,
    category: bool,
    name: SharedString,
    glyph: &'static str,
}

impl Render for ChanDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        div()
            .w(px(240.0))
            .h(px(32.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(radius_lg())
            .bg(p.background)
            .border_1()
            .border_color(alpha(p.primary, 0.5))
            .text_color(p.foreground)
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, 0.3),
                offset: gpui_kit::point(px(0.0), px(10.0)),
                blur_radius: px(24.0),
                spread_radius: px(-6.0),
                inset: false,
            }])
            .child(icon(self.glyph).size(px(16.0)).text_color(p.primary))
            .child(div().font_weight(FontWeight::BOLD).map(|el| {
                if self.category {
                    el.text_xs().child(self.name.to_uppercase())
                } else {
                    el.text_sm().child(self.name.clone())
                }
            }))
    }
}
