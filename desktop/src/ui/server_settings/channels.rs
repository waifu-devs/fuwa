//! The Channels page: every channel in order, moved a place at a time with
//! the arrows (into and out of categories too), and the chosen one beside the
//! list: its name, topic, category and slow mode, and who can see and do
//! what in it. The web's `settings/server/Channels.tsx` and
//! `ChannelPermissions.tsx`.

use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};

use super::roles::{dot, member_name, role_color, switch};
use super::*;
use crate::core::arrange::{self, Layout};
use crate::core::permissions::{self, Access, Bits, CHANNEL_GROUPS, bit};
use crate::core::server_admin::ChannelPatch;

/// Discord's slow mode stops, in seconds.
const SLOW: [i32; 14] = [0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];
/// How tall a channel row is, and the gap over a category.
const ROW: f32 = 36.0;
const CATEGORY_GAP: f32 = 10.0;
const VIEW: Bits = bit(P::ViewChannels);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Permissions,
    Share,
}

/// Where the role-or-person picker is open: under "who can see it", or beside Advanced.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Adding {
    Viewer,
    Target,
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
    if seconds == 0 { "Off".into() } else { crate::ui::moderate::duration(i64::from(seconds)) }
}

fn slow_short(seconds: i32) -> String {
    match seconds {
        s if s >= 3600 => format!("{}h", s / 3600),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
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

fn emerald(p: &Palette) -> Hsla {
    hsla(0.42, 0.65, if p.dark { 0.55 } else { 0.4 }, 1.0)
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
    /// The one that just moved, glowing for a moment.
    landed: Option<(String, Instant)>,
    /// Permissions not saved yet; `None` follows the channel as it is.
    draft: Option<Vec<Over>>,
    target: Option<String>,
    adding: Option<Adding>,
    perm_saving: bool,
}

impl Channels {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("channel-name"));
        let topic =
            cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 6).placeholder("What people talk about here"));
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
            landed: None,
            draft: None,
            target: None,
            adding: None,
            perm_saving: false,
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

    fn pick_channel(&mut self, id: String, cx: &mut Context<Self>) {
        let c = &mut self.channels;
        if c.selected.as_deref() != Some(id.as_str()) {
            c.selected = Some(id);
            c.parent = None;
            c.slowmode = None;
            c.confirming = false;
            c.draft = None;
            c.target = None;
            c.adding = None;
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
            self.error = Some("A channel needs a name.".into());
            cx.notify();
            return;
        }
        self.channels.saving = true;
        self.error = None;
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
                Err(err) => this.error = Some(err.message),
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
        self.error = None;
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

    fn step_channel(&mut self, layout: &Layout, id: String, by: i32, cx: &mut Context<Self>) {
        if self.channels.moving {
            return;
        }
        let Some(next) = arrange::step(layout, &id, by) else { return };
        self.channels.moving = true;
        self.channels.landed = Some((id, Instant::now()));
        // Let the glow fade once it's had its moment.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(950)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.reorder_channels(&key, &sid, next).await }, |this, result, cx| {
            this.channels.moving = false;
            if let Err(err) = result {
                this.error = Some(err.message);
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
        let editable = |id: &str| snap.access.has_in(id, P::ManageChannels) || snap.access.has_in(id, P::ManageRoles);

        // Keep a channel chosen that still exists.
        let first = layout
            .loose
            .iter()
            .chain(layout.categories.iter().flat_map(|(c, children)| std::iter::once(c).chain(children)))
            .find(|id| visible(id) && editable(id))
            .cloned();
        let selected = self.channels.selected.clone().filter(|id| by_id.contains_key(id.as_str())).or(first);
        if let Some(id) = &selected
            && self.channels.selected.as_deref() != Some(id.as_str())
        {
            self.pick_channel(id.clone(), cx);
        }
        if let Some((_, at)) = &self.channels.landed
            && at.elapsed() > Duration::from_millis(900)
        {
            self.channels.landed = None;
        }

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
            if y > 0.0 {
                y += CATEGORY_GAP;
            }
            entries.push((cat.clone(), true, y));
            y += ROW;
            for id in children.iter().filter(|id| visible(id)) {
                entries.push((id.clone(), false, y));
                y += ROW;
            }
        }
        let mut rows = div().relative().h(px(y));
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
            .w(px(280.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().text_xs().text_color(p.muted_foreground).child(if can_arrange {
                        "Move them with the arrows, into and out of categories."
                    } else {
                        "Pick a channel to change who can see and use it."
                    }))
                    .when(can_arrange, |el| {
                        el.child(
                            primary_button("channel-new", "New", p)
                                .h(px(34.0))
                                .px(px(12.0))
                                .child(icon("plus").size(px(15.0)))
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(ServerSettingsEvent::CreateChannel { parent: String::new() })
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .p(px(6.0))
                    .rounded(corner(16.0))
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.4))
                    .child(rows),
            );

        let editor = match selected.as_deref().and_then(|id| by_id.get(id)).map(|c| (*c).clone()) {
            Some(channel) => motion::rise(
                self.channel_editor(&channel, &snap, p, window, cx),
                SharedString::from(format!("chan-editor-{}", channel.id)),
                Duration::ZERO,
                10.0,
            )
            .into_any_element(),
            None => div()
                .py(px(40.0))
                .text_center()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("Pick a channel to change it.")
                .into_any_element(),
        };
        div()
            .flex()
            .items_start()
            .gap(px(28.0))
            .pb(px(80.0))
            .child(list)
            .child(div().flex_1().min_w_0().child(editor))
            .into_any_element()
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
    ) -> gpui_kit::Div {
        let hover = alpha(p.muted, 0.7);
        let mover = |glyph: &str, by: i32, cx: &mut Context<Self>| {
            let enabled = arrange::step(layout, &c.id, by).is_some();
            let (id, layout) = (c.id.clone(), layout.clone());
            let hover = alpha(p.muted_foreground, 0.14);
            div()
                .id(SharedString::from(format!("chan-{glyph}-{}", c.id)))
                .size(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(5.0))
                .text_color(alpha(p.muted_foreground, if enabled { 0.9 } else { 0.25 }))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.step_channel(&layout, id.clone(), by, cx)))
                })
                .child(icon(glyph).size(px(13.0)))
        };
        let handle = can_arrange.then(|| {
            div().w(px(18.0)).flex().flex_col().items_center().child(mover("chevron-up", -1, cx)).child(mover(
                "chevron-down",
                1,
                cx,
            ))
        });
        let private = permissions::is_private(c, &self.server);
        let landed = self.channels.landed.as_ref().is_some_and(|(id, _)| *id == c.id);
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
            .rounded(corner(10.0))
            .cursor_pointer()
            .when(active, |el| el.bg(alpha(p.primary, 0.12)).text_color(p.primary).font_weight(FontWeight::BOLD))
            .when(!active, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover)))
            .when(landed && !active, |el| el.bg(alpha(p.primary, 0.08)))
            .on_click(cx.listener(move |this, _, _, cx| this.pick_channel(id.clone(), cx)))
            .child(icon(glyph(c)).size(px(if category { 14.0 } else { 16.0 })))
            .child(div().min_w_0().truncate().map(|el| {
                if category {
                    el.text_xs().font_weight(FontWeight::EXTRA_BOLD).child(c.name.to_uppercase())
                } else {
                    el.text_sm().child(c.name.clone())
                }
            }));
        if private {
            pick = pick.child(motion::once(
                icon("lock").size(px(11.0)).text_color(p.muted_foreground),
                SharedString::from(format!("chan-lock-{}", c.id)),
                Duration::from_millis(380),
                |el, t| {
                    let s = 0.4 + 0.6 * t;
                    el.size(px(11.0 * s))
                },
            ));
        }
        pick = pick.child(div().flex_1());
        if !category && c.slowmode_seconds > 0 {
            pick = pick.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .text_size(px(11.0))
                    .text_color(p.muted_foreground)
                    .child(icon("snail").size(px(11.0)))
                    .child(slow_short(c.slowmode_seconds)),
            );
        }
        let add = (category && snap.access.has_in(&c.id, P::ManageChannels)).then(|| {
            let parent = c.id.clone();
            let fg = p.primary;
            div()
                .id(SharedString::from(format!("chan-add-{}", c.id)))
                .size(px(26.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(8.0))
                .cursor_pointer()
                .text_color(p.muted_foreground)
                .hover(move |s| s.text_color(fg))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ServerSettingsEvent::CreateChannel { parent: parent.clone() })
                }))
                .child(icon("plus").size(px(14.0)))
        });
        div()
            .h(px(ROW - 4.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .when(!category, |el| el.pl(px(if self.is_nested(c, layout) { 12.0 } else { 0.0 })))
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
            options.push((Tab::Overview, "Overview"));
        }
        if roles {
            options.push((Tab::Permissions, "Permissions"));
        }
        // Sharing takes managing the server and the channel, while the instance allows it (or it's shared already).
        let texty = matches!(kind(channel), pb::ChannelType::Text | pb::ChannelType::Announcement);
        let sharing_on = self
            .core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.node.as_ref()).is_some_and(|n| n.shared_channels));
        if texty && snap.access.has(P::ManageServer) && manage && (sharing_on || channel.shared.is_some()) {
            options.push((Tab::Share, "Share"));
        }
        let tab =
            options.iter().map(|(t, _)| *t).find(|t| *t == self.channels.tab).or(options.first().map(|(t, _)| *t));
        let mut tabs = div().flex().gap(px(4.0)).p(px(4.0)).rounded(corner(12.0)).bg(alpha(p.muted, 0.6));
        if options.len() > 1 {
            for (t, label) in options {
                let on = tab == Some(t);
                let hover = alpha(p.foreground, 0.06);
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!("chan-tab-{label}")))
                        .px(px(12.0))
                        .h(px(30.0))
                        .flex()
                        .items_center()
                        .rounded(corner(9.0))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .when(on, |el| el.bg(p.card).text_color(p.foreground))
                        .when(!on, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.channels.tab = t;
                            this.channels.adding = None;
                            cx.notify();
                        }))
                        .child(label),
                );
            }
        }
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.0))
            .mb(px(18.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_lg()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(icon(glyph(channel)).size(px(20.0)).text_color(p.muted_foreground))
                    .child(div().truncate().child(channel.name.clone())),
            )
            .child(tabs);
        let body = match tab {
            Some(Tab::Overview) => self.channel_overview(channel, snap, p, window, cx).into_any_element(),
            Some(Tab::Permissions) => self.channel_permissions(channel, snap, p, window, cx).into_any_element(),
            Some(Tab::Share) => self.channel_share(channel, p, window, cx),
            None => div()
                .py(px(40.0))
                .text_center()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("You can't change this one.")
                .into_any_element(),
        };
        div().flex().flex_col().child(header).child(motion::rise(
            div().child(body),
            SharedString::from(format!("chan-body-{}-{}", channel.id, tab.map(|t| t as u8).unwrap_or(9))),
            Duration::ZERO,
            8.0,
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
        let mut out = div().flex().flex_col().gap(px(20.0));

        let mut name = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(Input::new(&self.channels.name).when(!category, |el| el.prefix(icon("hash").size(px(15.0)))));
        if !will_be.is_empty() && will_be != typed.trim() {
            name = name
                .child(div().text_xs().text_color(p.muted_foreground).child(format!("It'll be saved as #{will_be}.")));
        }
        out = out.child(labeled(if category { "Category name" } else { "Channel name" }, name, p));

        if texty(channel) {
            out = out.child(labeled(
                "Topic",
                div().flex().flex_col().gap(px(6.0)).child(Textarea::new(&self.channels.topic)).child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child("Shown at the top of the channel. Markdown works."),
                ),
                p,
            ));
        }

        if !category {
            let parent = self.channels.parent.clone().unwrap_or_else(|| channel.parent_id.clone());
            let mut options = vec![(String::new(), "No category".to_owned())];
            options.extend(
                snap.channels
                    .iter()
                    .filter(|c| kind(c) == pb::ChannelType::Category)
                    .map(|c| (c.id.clone(), c.name.clone())),
            );
            out = out.child(labeled(
                "Category",
                text_chips("chan-parent", &options, &parent, p, cx, |this, id, cx| {
                    this.channels.parent = Some(id);
                    cx.notify();
                }),
                p,
            ));
        }

        if texty(channel) {
            let seconds = self.channels.slowmode.unwrap_or(channel.slowmode_seconds);
            let on = seconds > 0;
            // The snail crawls once each time it changes, quicker as it gets slower.
            let snail = div()
                .size(px(40.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(12.0))
                .bg(if on { alpha(p.primary, 0.15) } else { p.muted.into() })
                .text_color(if on { p.primary } else { p.muted_foreground })
                .child(motion::once(
                    icon("snail").size(px(20.0)),
                    SharedString::from(format!("chan-snail-{seconds}")),
                    Duration::from_millis(if on { 700 } else { 1 }),
                    move |el, t| {
                        let x = (t * std::f32::consts::PI).sin() * if on { 4.0 } else { 0.0 };
                        el.ml(px(x))
                    },
                ));
            out = out.child(labeled(
                "Slow mode",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .child(snail)
                            .child(div().flex_1().child(Slider::new(&self.channels.slow)))
                            .child(
                                div()
                                    .w(px(90.0))
                                    .text_right()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(slow_label(seconds)),
                            ),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).child(
                        "How long members wait between messages. People who can manage messages or channels here don't wait.",
                    )),
                p,
            ));
        }

        // Deleting, behind a confirmation.
        let id = channel.id.clone();
        let what = if category { channel.name.clone() } else { format!("#{}", channel.name) };
        let saving = self.channels.saving;
        let delete = if self.channels.confirming {
            let system = snap.system_channel == channel.id;
            motion::rise(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
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
                            .child(format!("Delete {what}?")),
                    )
                    .child(div().text_sm().text_color(p.muted_foreground).child(if category {
                        "Its channels stay, outside any category.".to_owned()
                    } else {
                        format!(
                            "Every message in it goes too, for everyone.{}",
                            if system { " Join messages stop until you pick another channel for them." } else { "" }
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(soft_button("chan-keep", "Keep it", p).on_click(cx.listener(|this, _, _, cx| {
                                this.channels.confirming = false;
                                cx.notify();
                            })))
                            .child(
                                danger_button("chan-delete-yes", if saving { "Deleting…" } else { "Delete" }, p)
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
            let fg = p.destructive;
            let hover = alpha(p.destructive, 0.1);
            div()
                .id("chan-delete")
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(36.0))
                .px(px(12.0))
                .rounded(corner(10.0))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.channels.confirming = true;
                    cx.notify();
                }))
                .child(icon("trash").size(px(15.0)))
                .child(if category { "Delete category" } else { "Delete channel" })
                .into_any_element()
        };
        out = out.child(div().flex().child(delete));

        let patch = self.channel_patch(channel, cx);
        let n =
            [patch.name.is_some(), patch.topic.is_some(), patch.parent_id.is_some(), patch.slowmode_seconds.is_some()]
                .into_iter()
                .filter(|c| *c)
                .count();
        if n > 0 {
            let c = channel.clone();
            self.bar = Some(save_bar(
                "chan-save-bar",
                n,
                self.channels.saving,
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

        let mut out = div().flex().flex_col().gap(px(18.0));

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
                    .rounded(corner(12.0))
                    .text_sm()
                    .bg(if synced { green.opacity(0.12) } else { alpha(p.muted, 0.6) })
                    .when(synced, |el| el.text_color(green))
                    .child(motion::once(
                        icon(if synced { "link" } else { "unlink" }).size(px(16.0)),
                        SharedString::from(format!("chan-sync-{synced}")),
                        Duration::from_millis(320),
                        |el, t| el.opacity(t),
                    ))
                    .child(div().flex_1().min_w_0().child(if synced {
                        format!("Same as its category, {}.", parent.name)
                    } else {
                        format!("Its own rules, apart from {}. Its category's apply first.", parent.name)
                    }))
                    .when(!synced, |el| {
                        el.child(soft_button("chan-sync", "Match the category", p).h(px(28.0)).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.channels.draft = Some(pb_over.clone());
                                cx.notify();
                            },
                        )))
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
            .rounded(corner(12.0))
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
            .rounded(corner(16.0))
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
                                "Private category"
                            } else {
                                "Private channel"
                            }))
                            .child(div().text_sm().text_color(p.muted_foreground).child(if category {
                                "Only the roles and people you pick see it and the channels that follow it."
                            } else {
                                "Only the roles and people you pick see it."
                            })),
                    )
                    .child(switch("chan-private".into(), private, !may(P::ViewChannels), cx, move |this, on, cx| {
                        this.put_over(&base_for_switch, &eid, false, |o| {
                            if on {
                                o.allow &= !VIEW;
                                o.deny |= VIEW;
                            } else {
                                o.deny &= !VIEW;
                            }
                        });
                        cx.notify();
                    })),
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
            let open = self.channels.adding == Some(Adding::Viewer);
            if may(P::ViewChannels) {
                let fg = p.primary;
                chips = chips.child(
                    div()
                        .id("chan-viewer-add")
                        .size(px(30.0))
                        .rounded_full()
                        .border_1()
                        .border_dashed()
                        .border_color(if open { p.primary } else { p.border })
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(if open { p.primary } else { p.muted_foreground })
                        .hover(move |s| s.text_color(fg).border_color(fg))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.channels.adding = if open { None } else { Some(Adding::Viewer) };
                            cx.notify();
                        }))
                        .child(icon(if open { "x" } else { "plus" }).size(px(14.0))),
                );
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
                        .child("WHO CAN SEE IT"),
                )
                .child(chips);
            if viewers.is_empty() {
                who = who.child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child("Nobody but the owner and administrators, for now."),
                );
            }
            if open {
                let roles: Vec<pb::Role> = snap
                    .roles
                    .iter()
                    .filter(|r| r.id != everyone_id && find(&r.id).is_none_or(|o| o.allow & VIEW == 0))
                    .cloned()
                    .collect();
                let people: Vec<pb::Member> = snap
                    .members
                    .iter()
                    .filter(|m| m.user.as_ref().is_some_and(|u| find(&u.id).is_none_or(|o| o.allow & VIEW == 0)))
                    .cloned()
                    .collect();
                who = who.child(self.add_picker("chan-pick-viewer", &roles, &people, true, &base, p, cx));
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
                    .rounded(corner(12.0))
                    .border_1()
                    .border_color(warn.opacity(0.4))
                    .bg(warn.opacity(0.1))
                    .text_sm()
                    .text_color(warn)
                    .child(icon("triangle-alert").size(px(16.0)))
                    .child(div().flex_1().min_w_0().child(format!(
                        "Saving this hides the {} from you too. Add one of your roles to keep it.",
                        if category { "category" } else { "channel" }
                    ))),
                "chan-losing",
                Duration::ZERO,
                -6.0,
            ));
        }

        // Advanced: every channel permission, per role or person.
        let open = self.channels.adding == Some(Adding::Target);
        let mut advanced = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Advanced permissions"))
                        .child(div().text_sm().text_color(p.muted_foreground).child(
                            "Allow or deny each permission here for a role or a person. The rest follows their roles.",
                        )),
                )
                .child(
                    soft_button("chan-target-add", if open { "Close" } else { "Add" }, p)
                        .child(icon(if open { "x" } else { "plus" }).size(px(14.0)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.channels.adding = if open { None } else { Some(Adding::Target) };
                            cx.notify();
                        })),
                ),
        );
        if open {
            let roles: Vec<pb::Role> =
                snap.roles.iter().filter(|r| r.id != everyone_id && find(&r.id).is_none()).cloned().collect();
            let people: Vec<pb::Member> = snap
                .members
                .iter()
                .filter(|m| m.user.as_ref().is_some_and(|u| find(&u.id).is_none()))
                .cloned()
                .collect();
            advanced = advanced.child(self.add_picker("chan-pick-target", &roles, &people, false, &base, p, cx));
        }

        let selected = self
            .channels
            .target
            .clone()
            .filter(|id| *id == everyone_id || targets.iter().any(|o| o.target_id == *id))
            .unwrap_or_else(|| everyone_id.clone());
        let mut side = div().w(px(190.0)).flex_none().flex().flex_col().gap(px(2.0));
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
                    .rounded(corner(10.0))
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
            .rounded(corner(16.0))
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
                    .child(title.to_uppercase()),
            );
            for perm in list.iter().copied() {
                let (label, _) = permissions::info(perm);
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
                            .child(
                                div().text_xs().text_color(p.muted_foreground).child(permissions::channel_about(perm)),
                            ),
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
                        .rounded(corner(10.0))
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
                        .child(if category { "Remove from this category" } else { "Remove from this channel" }),
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
                None => row.child(icon("user").size(px(15.0))).child(div().truncate().child("Someone who left")),
            }
        } else {
            row.child(dot(role.and_then(role_color), 10.0, p))
                .child(div().truncate().child(role.map(|r| r.name.clone()).unwrap_or_else(|| "A deleted role".into())))
        }
    }

    /// Roles and people to add, as chips; picking one adds them and closes it.
    #[allow(clippy::too_many_arguments)]
    fn add_picker(
        &self,
        id: &'static str,
        roles: &[pb::Role],
        people: &[pb::Member],
        viewer: bool,
        base: &[Over],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chip_of = |key: String, label: gpui_kit::Div, target: String, member: bool, cx: &mut Context<Self>| {
            let base = base.to_vec();
            let hover = mix(p.secondary, p.primary, 0.16);
            div()
                .id(SharedString::from(format!("{id}-{key}")))
                .h(px(30.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.secondary)
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(|s| s.top(px(1.0)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.put_over(&base, &target, member, |o| {
                        if viewer {
                            o.allow |= VIEW;
                            o.deny &= !VIEW;
                        }
                    });
                    this.channels.target = Some(target.clone());
                    this.channels.adding = None;
                    cx.notify();
                }))
                .child(label)
        };
        let mut out = div().flex().flex_col().gap(px(8.0)).p(px(12.0)).rounded(corner(14.0)).bg(alpha(p.muted, 0.5));
        if roles.is_empty() && people.is_empty() {
            out = out.child(div().text_sm().text_color(p.muted_foreground).child("Everyone's here already."));
        }
        if !roles.is_empty() {
            let mut row = div().flex().flex_wrap().gap(px(6.0));
            for r in roles {
                let label =
                    div().flex().items_center().gap(px(6.0)).child(dot(role_color(r), 10.0, p)).child(r.name.clone());
                row = row.child(chip_of(r.id.clone(), label, r.id.clone(), false, cx));
            }
            out = out
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.muted_foreground)
                        .child("ROLES"),
                )
                .child(row);
        }
        if !people.is_empty() {
            let mut row = div().flex().flex_wrap().gap(px(6.0));
            for m in people.iter().take(30) {
                let Some(u) = &m.user else { continue };
                let label =
                    div().flex().items_center().gap(px(6.0)).child(avatar(Some(u), 18.0, p)).child(member_name(m));
                row = row.child(chip_of(u.id.clone(), label, u.id.clone(), true, cx));
            }
            out = out
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.muted_foreground)
                        .child("PEOPLE"),
                )
                .child(row);
        }
        motion::rise(out, SharedString::from(format!("{id}-in")), Duration::ZERO, -6.0).into_any_element()
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
            1 => emerald(p),
            -1 => p.destructive.into(),
            _ => alpha(p.muted_foreground, 0.3),
        };
        let mut out = div()
            .relative()
            .flex_none()
            .flex()
            .p(px(2.0))
            .rounded(corner(9.0))
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.muted, 0.4))
            .when(disabled, |el| el.opacity(0.5))
            .child(div().absolute().top(px(2.0)).left(px(2.0 + at)).size(px(CELL)).rounded(corner(7.0)).bg(tint));
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
                        icon(glyph).size(px(14.0)),
                        SharedString::from(format!("tri-pop-{}-{}-{value}-{on}", o.target_id, perm as i32)),
                        Duration::from_millis(if on { 260 } else { 1 }),
                        move |el, t| {
                            let s = if on { 0.6 + 0.4 * t + 0.25 * (t * std::f32::consts::PI).sin() } else { 1.0 };
                            el.size(px(14.0 * s))
                        },
                    )),
            );
        }
        out
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
