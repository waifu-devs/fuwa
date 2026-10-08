//! The Roles page: every role, highest first, dragged into rank by its handle
//! (only those below your own move), and the chosen one beside the list: how it looks,
//! what it can do and who has it. @everyone sits at the bottom and holds what
//! everybody can do. The web's `settings/server/Roles.tsx`.

use std::rc::Rc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::rgb;

use gpui_kit::{Render, Stateful, point};

use super::pages::form_row;
use super::*;
use crate::core::permissions::{self, Access, Bits, GROUPS, bit};
use crate::core::server_admin::RolePatch;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{radius_2xl, radius_lg, radius_md, radius_xl};

/// Colors to pick from, light to deep, as on the web.
const SWATCHES: [u32; 24] = [
    0xf472b6, 0xfb7185, 0xf87171, 0xfb923c, 0xfbbf24, 0xa3e635, 0x34d399, 0x2dd4bf, 0x38bdf8, 0x60a5fa, 0x818cf8,
    0xa78bfa, 0xe879f9, 0xdb2777, 0xe11d48, 0xea580c, 0xca8a04, 0x16a34a, 0x0d9488, 0x0284c7, 0x4f46e5, 0x7c3aed,
    0x94a3b8, 0x64748b,
];

/// A permission's name, as the app shows it.
pub(super) fn permission_name(p: P) -> String {
    match p {
        P::Unspecified => String::new(),
        P::Administrator => t("serversettings.permission.administrator"),
        P::ManageServer => t("serversettings.permission.manageServer"),
        P::ManageRoles => t("serversettings.permission.manageRoles"),
        P::ViewAuditLog => t("serversettings.permission.viewAuditLog"),
        P::ChangeNickname => t("serversettings.permission.changeNickname"),
        P::ManageNicknames => t("serversettings.permission.manageNicknames"),
        P::KickMembers => t("serversettings.permission.kickMembers"),
        P::BanMembers => t("serversettings.permission.banMembers"),
        P::TimeOutMembers => t("serversettings.permission.timeOutMembers"),
        P::ManageChannels => t("serversettings.permission.manageChannels"),
        P::ViewChannels => t("serversettings.permission.viewChannels"),
        P::SendMessages => t("serversettings.permission.sendMessages"),
        P::CreateThreads => t("serversettings.permission.createThreads"),
        P::CreatePolls => t("serversettings.permission.createPolls"),
        P::EmbedLinks => t("serversettings.permission.embedLinks"),
        P::AttachFiles => t("serversettings.permission.attachFiles"),
        P::MentionEveryone => t("serversettings.permission.mentionEveryone"),
        P::ManageMessages => t("serversettings.permission.manageMessages"),
        P::CreateInvite => t("serversettings.permission.createInvite"),
        P::ManageEmoji => t("serversettings.permission.manageEmoji"),
        P::ManageWebhooks => t("serversettings.permission.manageWebhooks"),
        P::Connect => t("serversettings.permission.connect"),
        P::Speak => t("serversettings.permission.speak"),
        P::Video => t("serversettings.permission.video"),
        P::Record => t("serversettings.permission.record"),
        P::MuteMembers => t("serversettings.permission.muteMembers"),
        P::MoveMembers => t("serversettings.permission.moveMembers"),
    }
}

/// What a permission lets people do, server-wide.
pub(super) fn permission_about(p: P) -> String {
    match p {
        P::Unspecified => String::new(),
        P::Administrator => t("serversettings.permission.administratorAbout"),
        P::ManageServer => t("serversettings.permission.manageServerAbout"),
        P::ManageRoles => t("serversettings.permission.manageRolesAbout"),
        P::ViewAuditLog => t("serversettings.permission.viewAuditLogAbout"),
        P::ChangeNickname => t("serversettings.permission.changeNicknameAbout"),
        P::ManageNicknames => t("serversettings.permission.manageNicknamesAbout"),
        P::KickMembers => t("serversettings.permission.kickMembersAbout"),
        P::BanMembers => t("serversettings.permission.banMembersAbout"),
        P::TimeOutMembers => t("serversettings.permission.timeOutMembersAbout"),
        P::ManageChannels => t("serversettings.permission.manageChannelsAbout"),
        P::ViewChannels => t("serversettings.permission.viewChannelsAbout"),
        P::SendMessages => t("serversettings.permission.sendMessagesAbout"),
        P::CreateThreads => t("serversettings.permission.createThreadsAbout"),
        P::CreatePolls => t("serversettings.permission.createPollsAbout"),
        P::EmbedLinks => t("serversettings.permission.embedLinksAbout"),
        P::AttachFiles => t("serversettings.permission.attachFilesAbout"),
        P::MentionEveryone => t("serversettings.permission.mentionEveryoneAbout"),
        P::ManageMessages => t("serversettings.permission.manageMessagesAbout"),
        P::CreateInvite => t("serversettings.permission.createInviteAbout"),
        P::ManageEmoji => t("serversettings.permission.manageEmojiAbout"),
        P::ManageWebhooks => t("serversettings.permission.manageWebhooksAbout"),
        P::Connect => t("serversettings.permission.connectAbout"),
        P::Speak => t("serversettings.permission.speakAbout"),
        P::Video => t("serversettings.permission.videoAbout"),
        P::Record => t("serversettings.permission.recordAbout"),
        P::MuteMembers => t("serversettings.permission.muteMembersAbout"),
        P::MoveMembers => t("serversettings.permission.moveMembersAbout"),
    }
}

/// What a permission lets people do in one channel, where that reads differently from server-wide.
pub(super) fn permission_here(p: P) -> String {
    match p {
        P::ManageRoles => t("serversettings.permission.manageRolesChannel"),
        P::ManageChannels => t("serversettings.permission.manageChannelsChannel"),
        P::ViewChannels => t("serversettings.permission.viewChannelsChannel"),
        P::CreatePolls => t("serversettings.permission.createPollsChannel"),
        P::CreateInvite => t("serversettings.permission.createInviteChannel"),
        P::Connect => t("serversettings.permission.connectChannel"),
        P::Speak => t("serversettings.permission.speakChannel"),
        P::Video => t("serversettings.permission.videoChannel"),
        P::Record => t("serversettings.permission.recordChannel"),
        P::MuteMembers => t("serversettings.permission.muteMembersChannel"),
        P::MoveMembers => t("serversettings.permission.moveMembersChannel"),
        other => permission_about(other),
    }
}

/// A permission group's title, by the name `core::permissions` gives it.
pub(super) fn group_name(title: &str) -> String {
    match title {
        "Server" => t("serversettings.permission.group.server"),
        "Membership" => t("serversettings.permission.group.membership"),
        "Text channels" => t("serversettings.permission.group.textChannels"),
        "Voice channels" => t("serversettings.permission.group.voiceChannels"),
        "Advanced" => t("serversettings.permission.group.advanced"),
        "General" => t("serversettings.permission.group.general"),
        "Text" => t("serversettings.permission.group.text"),
        "Voice" => t("serversettings.permission.group.voice"),
        other => other.to_owned(),
    }
}

/// How tall a row in the role list is, gap included.
const ROW: f32 = 38.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RoleTab {
    Display,
    Permissions,
    Members,
}

/// What was changed and not saved yet; `None` follows the role as it is.
#[derive(Default, Clone)]
struct Edits {
    color: Option<Option<u32>>,
    hoist: Option<bool>,
    mentionable: Option<bool>,
    /// Permissions switched on and off here. Kept apart from the role's own,
    /// so a change someone else saves meanwhile shows and isn't undone.
    grant: Bits,
    revoke: Bits,
}

impl Edits {
    /// The role's permissions as they are, with the switches flipped here on top.
    fn permissions(&self, role: &pb::Role) -> Bits {
        (permissions::from_list(&role.permissions) | self.grant) & !self.revoke
    }

    /// A switch put back as the role has it (`live`) isn't being changed any more.
    fn switch(&mut self, live: Bits, bits: Bits, on: bool) {
        if on {
            self.grant |= bits;
            self.revoke &= !bits;
        } else {
            self.revoke |= bits;
            self.grant &= !bits;
        }
        self.grant &= !live;
        self.revoke &= live;
    }

    /// What saving turns on and off in the role as it is.
    fn changes(&self, role: &pb::Role) -> (Bits, Bits) {
        let live = permissions::from_list(&role.permissions);
        (self.grant & !live, self.revoke & live)
    }
}

pub(super) struct Roles {
    selected: Option<String>,
    tab: RoleTab,
    edits: Edits,
    name: Entity<InputState>,
    /// What the name box was filled with, to tell typing from a rename made elsewhere.
    name_base: String,
    /// Which role the boxes were filled for.
    filled_for: Option<String>,
    perm_query: Entity<InputState>,
    member_query: Entity<InputState>,
    adding: bool,
    confirming: bool,
    saving: bool,
    /// Who's being given or losing the role, or "new" while a role is made.
    busy: Option<String>,
    /// The order being saved, shown until the server answers.
    moving: Option<Vec<String>>,
    saved: Option<Instant>,
    /// Picking any color: the box for its hex code is open.
    custom: bool,
    hex: Entity<InputState>,
    /// What went wrong with the last save, said in the save bar.
    error: Option<String>,
}

impl Roles {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.roles.newRole")));
        let perm_query = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.roles.search")));
        let member_query = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.shared.findSomeone")));
        let hex = cx.new(|cx| InputState::new(window, cx).placeholder("#f472b6"));
        let mut subscriptions: Vec<Subscription> = [&name, &perm_query, &member_query]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                    if let InputEvent::Change = e {
                        cx.notify()
                    }
                })
            })
            .collect();
        subscriptions.push(cx.subscribe(&hex, |this: &mut ServerSettingsView, hex, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                let text = hex.read(cx).value().trim().trim_start_matches('#').to_string();
                if text.len() == 6
                    && let Ok(c) = u32::from_str_radix(&text, 16)
                {
                    this.roles.edits.color = Some(Some(c));
                }
                cx.notify()
            }
        }));
        let roles = Self {
            selected: None,
            tab: RoleTab::Display,
            edits: Edits::default(),
            name,
            name_base: String::new(),
            filled_for: None,
            perm_query,
            member_query,
            adding: false,
            confirming: false,
            saving: false,
            busy: None,
            moving: None,
            saved: None,
            custom: false,
            hex,
            error: None,
        };
        (roles, subscriptions)
    }
}

/// The page's view of the server.
struct Snap {
    /// Highest first, @everyone last.
    roles: Vec<pb::Role>,
    members: Vec<pb::Member>,
    access: Access,
    me: Option<pb::User>,
}

fn color_of(c: u32) -> Hsla {
    rgb(c).into()
}

pub(super) fn role_color(r: &pb::Role) -> Option<u32> {
    r.color.map(|c| c as u32)
}

pub(super) fn member_name(m: &pb::Member) -> String {
    match &m.user {
        Some(u) if m.nickname.is_empty() => user_name(u),
        Some(_) => m.nickname.clone(),
        None => String::new(),
    }
}

/// A small dot in a role's color, or an outline for one without.
pub(super) fn dot(color: Option<u32>, size: f32, p: &Palette) -> gpui_kit::Div {
    let d = div().flex_none().size(px(size)).rounded_full();
    match color {
        Some(c) => d.bg(color_of(c)),
        None => d.border_2().border_color(alpha(p.muted_foreground, 0.6)),
    }
}

/// A switch that calls back into the page.
pub(crate) fn switch<V: 'static>(
    id: SharedString,
    on: bool,
    disabled: bool,
    cx: &mut Context<V>,
    set: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let set = Rc::new(set);
    Switch::new(id).checked(on).disabled(disabled).on_change(move |checked, _, cx| {
        let (set, checked) = (set.clone(), *checked);
        let _ = entity.update(cx, |this, cx| set(this, checked, cx));
    })
}

impl ServerSettingsView {
    fn snap(&self) -> Snap {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else {
                return Snap { roles: Vec::new(), members: Vec::new(), access: Access::default(), me: None };
            };
            Snap {
                roles: i.roles.get(&self.server).cloned().unwrap_or_default(),
                members: i.members.get(&self.server).cloned().unwrap_or_default(),
                access: i.access(&self.server),
                me: i.me.clone(),
            }
        })
    }

    fn select_role(&mut self, id: String, cx: &mut Context<Self>) {
        let r = &mut self.roles;
        if r.selected.as_deref() != Some(id.as_str()) {
            r.selected = Some(id);
            r.edits = Edits::default();
            r.confirming = false;
            r.adding = false;
            r.saved = None;
            self.error = None;
        }
        cx.notify();
    }

    /// Fills the name box for the chosen role, and follows a rename made
    /// elsewhere while nothing was typed.
    fn fill_role(&mut self, role: &pb::Role, window: &mut Window, cx: &mut Context<Self>) {
        let r = &mut self.roles;
        let typed = r.name.read(cx).value().to_string();
        let fresh = r.filled_for.as_deref() != Some(role.id.as_str());
        if fresh || (typed == r.name_base && role.name != r.name_base) {
            r.filled_for = Some(role.id.clone());
            r.name_base = role.name.clone();
            let name = role.name.clone();
            r.name.update(cx, |s, cx| s.set_value(name, window, cx));
        }
    }

    fn new_role(&mut self, cx: &mut Context<Self>) {
        if self.roles.busy.is_some() {
            return;
        }
        self.roles.busy = Some("new".into());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let name = t("serversettings.roles.newRole");
        self.run(cx, async move { core.create_role(&key, &sid, &name).await }, |this, result, cx| {
            this.roles.busy = None;
            match result {
                Ok(role) => {
                    this.select_role(role.id, cx);
                    this.roles.tab = RoleTab::Display;
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn changes(&self, role: &pb::Role, everyone: bool, cx: &Context<Self>) -> usize {
        let e = &self.roles.edits;
        let name = self.roles.name.read(cx).value().trim().to_string();
        [
            !everyone && self.roles.filled_for.as_deref() == Some(role.id.as_str()) && name != role.name,
            e.color.is_some_and(|c| c != role_color(role)),
            e.hoist.is_some_and(|h| h != role.hoist),
            e.mentionable.is_some_and(|m| m != role.mentionable),
            e.changes(role) != (0, 0),
        ]
        .into_iter()
        .filter(|c| *c)
        .count()
    }

    fn save_role(&mut self, role: &pb::Role, everyone: bool, cx: &mut Context<Self>) {
        let e = self.roles.edits.clone();
        let name = self.roles.name.read(cx).value().trim().to_string();
        if !everyone && name.is_empty() {
            self.roles.error = Some(t("serversettings.roles.needsName"));
            cx.notify();
            return;
        }
        let (grant, revoke) = e.changes(role);
        let changes =
            self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.has("role-permission-changes")));
        // Older instances only take every permission at once.
        let whole = (!changes && (grant, revoke) != (0, 0)).then(|| permissions::to_list(e.permissions(role)));
        let (grant, revoke) = if changes { (grant, revoke) } else { (0, 0) };
        let patch = RolePatch {
            name: (!everyone && name != role.name).then_some(name),
            color: e.color.filter(|c| *c != role_color(role)),
            hoist: e.hoist.filter(|h| *h != role.hoist),
            mentionable: e.mentionable.filter(|m| *m != role.mentionable),
            permissions: whole,
            grant: permissions::to_list(grant),
            revoke: permissions::to_list(revoke),
        };
        self.roles.saving = true;
        self.roles.error = None;
        let (core, key, sid, rid) = (self.core.clone(), self.key.clone(), self.server.clone(), role.id.clone());
        self.run(cx, async move { core.update_role(&key, &sid, &rid, patch).await }, |this, result, cx| {
            this.roles.saving = false;
            match result {
                Ok(role) => {
                    this.roles.edits = Edits::default();
                    this.roles.name_base = role.name;
                    this.roles.saved = Some(Instant::now());
                    this.flash_saved(cx);
                }
                Err(err) => this.roles.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn discard_role(&mut self, role: &pb::Role, window: &mut Window, cx: &mut Context<Self>) {
        self.roles.edits = Edits::default();
        self.roles.filled_for = None;
        self.roles.error = None;
        self.fill_role(role, window, cx);
        cx.notify();
    }

    fn delete_role(&mut self, role_id: String, cx: &mut Context<Self>) {
        self.roles.saving = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.delete_role(&key, &sid, &role_id).await }, |this, result, cx| {
            this.roles.saving = false;
            this.roles.confirming = false;
            match result {
                Ok(()) => {
                    this.roles.selected = None;
                    this.roles.edits = Edits::default();
                }
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn give_role(&mut self, user_id: String, role_id: String, give: bool, cx: &mut Context<Self>) {
        if self.roles.busy.is_some() {
            return;
        }
        self.roles.busy = Some(user_id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.set_member_role(&key, &sid, &user_id, &role_id, give).await },
            |this, result, cx| {
                this.roles.busy = None;
                if let Err(err) = result {
                    this.error = Some(err.message);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn roles_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.snap();
        let everyone_id = self.server.clone();
        let ranked: Vec<&pb::Role> = snap.roles.iter().filter(|r| r.id != everyone_id).collect();
        let order: Vec<String> = match &self.roles.moving {
            Some(order) => order.clone(),
            None => ranked.iter().map(|r| r.id.clone()).collect(),
        };
        let by_id: HashMap<&str, &pb::Role> = snap.roles.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for m in &snap.members {
            for id in &m.role_ids {
                *counts.entry(id.as_str()).or_default() += 1;
            }
        }

        // Keep a role chosen that still exists.
        let selected = self
            .roles
            .selected
            .clone()
            .filter(|id| by_id.contains_key(id.as_str()))
            .or_else(|| ranked.first().map(|r| r.id.clone()))
            .unwrap_or_else(|| everyone_id.clone());
        if self.roles.selected.as_deref() != Some(selected.as_str()) {
            self.select_role(selected.clone(), cx);
        }
        let role = by_id.get(selected.as_str()).map(|r| (*r).clone());
        let everyone = selected == everyone_id;
        if everyone && self.roles.tab == RoleTab::Members {
            self.roles.tab = RoleTab::Permissions;
        }
        if everyone && self.roles.tab == RoleTab::Display {
            self.roles.tab = RoleTab::Permissions;
        }
        if let Some(role) = &role {
            self.fill_role(role, window, cx);
        }

        // The list, each row sliding to its place when the order changes; the highlight glides.
        let mut rows = div().relative().h(px(ROW * order.len() as f32 - 2.0));
        let active_at = order.iter().position(|id| *id == selected);
        if let Some(at) = active_at {
            let y = motion::follow("role-hl", at as f32 * ROW, window, cx);
            rows = rows.child(
                div()
                    .absolute()
                    .left(px(30.0))
                    .right_0()
                    .top(px(y))
                    .h(px(36.0))
                    .rounded(radius_lg())
                    .bg(alpha(p.primary, 0.12)),
            );
        }
        for (n, id) in order.iter().enumerate() {
            let Some(r) = by_id.get(id.as_str()) else { continue };
            let y = motion::follow(SharedString::from(format!("role-y-{id}")), n as f32 * ROW, window, cx);
            let active = *id == selected;
            let locked = !snap.access.above(r.position);
            let row = self.role_row(r, counts.get(id.as_str()).copied().unwrap_or(0), active, locked, n, &order, p, cx);
            rows = rows.child(div().absolute().left_0().right_0().top(px(y)).child(motion::rise(
                row,
                SharedString::from(format!("role-in-{id}")),
                Duration::from_millis(25 * n.min(12) as u64),
                6.0,
            )));
        }
        let everyone_row = {
            let on = everyone;
            let (hover, fg) = (alpha(p.muted, 0.7), p.foreground);
            div()
                .id("role-everyone")
                .relative()
                .mt(px(8.0))
                .h(px(44.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .text_sm()
                .cursor_pointer()
                .when(on, |el| el.bg(alpha(p.primary, 0.12)).text_color(p.primary).font_weight(FontWeight::BOLD))
                .when(!on, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover).text_color(fg)))
                .on_click({
                    let id = everyone_id.clone();
                    cx.listener(move |this, _, _, cx| this.select_role(id.clone(), cx))
                })
                .child(icon("users").size(px(16.0)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().line_height(px(20.0)).child("@everyone"))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.2))
                                .line_height(px(15.0))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.roles.everyoneHint")),
                        ),
                )
        };
        let can_create = snap.access.has(P::ManageRoles);
        let making = self.roles.busy.as_deref() == Some("new");
        let list = div()
            .w(px(272.0))
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
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.roles.intro")),
                    )
                    .when(can_create, |el| {
                        el.child(
                            button(
                                "role-new",
                                t("serversettings.shared.new"),
                                if making { None } else { Some("plus") },
                                Look::Primary,
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .when(making, |el| el.opacity(0.6).child(spinner("role-new-spin", 16.0, window)))
                            .when(!making, |el| el.on_click(cx.listener(|this, _, _, cx| this.new_role(cx)))),
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
                    .child(rows)
                    .child(everyone_row),
            );

        let editor = match role {
            Some(role) => motion::slide_in(
                self.role_editor(&role, everyone, &snap, p, window, cx),
                SharedString::from(format!("role-editor-{}", role.id)),
                16.0,
            )
            .into_any_element(),
            None => div()
                .py(px(40.0))
                .text_center()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t("serversettings.roles.pick"))
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

    /// Drops a role being dragged at `to` in the order, if you rank above every role it passes.
    fn drop_role(&mut self, order: &[String], id: &str, to: usize, cx: &mut Context<Self>) {
        let Some(at) = order.iter().position(|r| r == id) else { return };
        if at == to || self.roles.moving.is_some() {
            return;
        }
        let snap = self.snap();
        let passes = if to < at { &order[to..at] } else { &order[at + 1..=to.min(order.len() - 1)] };
        let mine = passes
            .iter()
            .all(|o| snap.roles.iter().find(|r| &r.id == o).is_some_and(|r| snap.access.above(r.position)));
        if !mine {
            return;
        }
        let mut next = order.to_vec();
        let moved = next.remove(at);
        next.insert(to.min(next.len()), moved);
        self.roles.moving = Some(next.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.reorder_roles(&key, &sid, next).await }, |this, result, cx| {
            this.roles.moving = None;
            if let Err(err) = result {
                cx.emit(ServerSettingsEvent::Toast { icon: "circle-alert", title: err.message });
            }
            cx.notify();
        });
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn role_row(
        &self,
        r: &pb::Role,
        count: usize,
        active: bool,
        locked: bool,
        n: usize,
        order: &[String],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui_kit::Div> {
        let color = role_color(r);
        let (hover, fg) = (alpha(p.muted, 0.7), p.foreground);
        let handle = if locked {
            div()
                .size(px(28.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_color(alpha(p.muted_foreground, 0.5))
                .child(icon("lock").size(px(14.0)))
                .into_any_element()
        } else {
            let drag = RoleDrag { id: r.id.clone(), name: r.name.clone().into(), color };
            let (bg, fg) = (p.muted, p.foreground);
            div()
                .id(SharedString::from(format!("role-grip-{}", r.id)))
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
                .into_any_element()
        };
        let id = r.id.clone();
        let order = order.to_vec();
        let drop_hl = alpha(p.primary, 0.08);
        div()
            .id(SharedString::from(format!("role-row-{}", r.id)))
            .h(px(36.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .rounded(radius_lg())
            .drag_over::<RoleDrag>(move |s, _, _, _| s.bg(drop_hl))
            .on_drop::<RoleDrag>(
                cx.listener(move |this, drag: &RoleDrag, _, cx| this.drop_role(&order, &drag.id, n, cx)),
            )
            .child(handle)
            .child(
                div()
                    .id(SharedString::from(format!("role-pick-{}", r.id)))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .cursor_pointer()
                    .text_sm()
                    .when(active, |el| el.font_weight(FontWeight::BOLD))
                    .when(!active, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover).text_color(fg)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_role(id.clone(), cx)))
                    .child(motion::once(
                        dot(color, 12.0, p),
                        SharedString::from(format!("role-dot-{}-{color:?}", r.id)),
                        Duration::from_millis(300),
                        |el, t| el.opacity(0.3 + 0.7 * t),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when_some(color.filter(|_| active), |el, c| el.text_color(color_of(c)))
                            .child(r.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(11.2))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(icon("users").size(px(12.0)))
                            .child(count.to_string()),
                    ),
            )
    }

    fn role_editor(
        &mut self,
        role: &pb::Role,
        everyone: bool,
        snap: &Snap,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let locked = !everyone && !snap.access.above(role.position);
        let e = self.roles.edits.clone();
        let color = e.color.unwrap_or(role_color(role));
        let typed = self.roles.name.read(cx).value().trim().to_string();
        let shown = if typed.is_empty() { t("serversettings.roles.newRole") } else { typed };
        let holders = snap.members.iter().filter(|m| m.role_ids.contains(&role.id)).count();

        let options: Vec<(RoleTab, String)> = if everyone {
            vec![(RoleTab::Permissions, t("serversettings.shared.permissions"))]
        } else {
            vec![
                (RoleTab::Display, t("serversettings.roles.display")),
                (RoleTab::Permissions, t("serversettings.shared.permissions")),
                (RoleTab::Members, t_with("serversettings.roles.membersTab", &[("count", Arg::Num(holders as i64))])),
            ]
        };
        let chosen = options.iter().position(|(t, _)| *t == self.roles.tab).unwrap_or(0);
        let tabs_of: Vec<RoleTab> = options.iter().map(|(t, _)| *t).collect();
        let labels: Vec<String> = options.into_iter().map(|(_, l)| l).collect();
        let tabs = (labels.len() > 1).then(|| {
            self.seg_tabs("role-tabs", labels, chosen, p, window, cx, move |this, n, cx| {
                this.roles.tab = tabs_of[n];
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
                    .when(!everyone, |el| {
                        el.child(motion::once(
                            dot(color, 16.0, p),
                            SharedString::from(format!("role-title-dot-{color:?}")),
                            Duration::from_millis(350),
                            |el, t| el.opacity(t),
                        ))
                    })
                    .child(
                        div()
                            .truncate()
                            .when_some(color.filter(|_| !everyone), |el, c| el.text_color(color_of(c)))
                            .child(if everyone { "@everyone".to_owned() } else { shown.clone() }),
                    ),
            )
            .children(tabs);

        let tab = self.roles.tab;
        let body = match tab {
            RoleTab::Display if !everyone => self.role_display(role, &e, color, &shown, locked, snap, p, window, cx),
            RoleTab::Members if !everyone => self.role_members(role, locked, snap, p, window, cx),
            _ => self.role_permissions(role, &e, everyone, locked, &snap.access, p, window, cx),
        };

        let n = self.changes(role, everyone, cx);
        let saving = self.roles.saving;
        if n > 0 {
            let (r1, r2) = (role.clone(), role.clone());
            self.bar = Some(bar_with_error(
                "role-save-bar",
                n,
                saving,
                self.roles.error.as_deref(),
                p,
                cx,
                move |this, window, cx| this.discard_role(&r1, window, cx),
                move |this, _, cx| {
                    if !this.roles.saving {
                        this.save_role(&r2, everyone, cx)
                    }
                },
            ));
        }
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(locked, |el| {
                el.child(super::pages::slide_in(
                    div()
                        .mb(px(16.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(radius_xl())
                        .bg(alpha(p.muted, 0.6))
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(icon("lock").size(px(16.0)))
                        .child(t("serversettings.roles.locked")),
                    "role-locked",
                ))
            })
            .child(motion::rise(
                body,
                SharedString::from(format!("role-body-{}-{}", role.id, tab as u8)),
                Duration::ZERO,
                10.0,
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn role_display(
        &mut self,
        role: &pb::Role,
        e: &Edits,
        color: Option<u32>,
        shown: &str,
        locked: bool,
        snap: &Snap,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        // A ring around the picked swatch (`ring-2 ring-foreground ring-offset-2`).
        let ring = |el: Stateful<gpui_kit::Div>, on: bool| {
            el.when(on, |el| {
                el.shadow(vec![
                    gpui_kit::BoxShadow {
                        color: p.foreground.into(),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(4.0),
                        inset: false,
                    },
                    gpui_kit::BoxShadow {
                        color: p.background.into(),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: px(2.0),
                        inset: false,
                    },
                ])
            })
        };
        let mut swatches = div().flex().flex_wrap().gap(px(8.0));
        for c in std::iter::once(None).chain(SWATCHES.iter().map(|c| Some(*c))) {
            let on = color == c;
            let mut swatch = div()
                .id(SharedString::from(format!("swatch-{c:?}")))
                .relative()
                .size(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .when(locked, |el| el.opacity(0.5))
                .when(!locked, |el| {
                    el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                        this.roles.edits.color = Some(c);
                        this.roles.custom = false;
                        cx.notify();
                    }))
                });
            swatch = match c {
                Some(c) => swatch.bg(color_of(c)),
                None => swatch.border_2().border_color(alpha(p.muted_foreground, 0.4)).bg(p.background),
            };
            swatch = ring(swatch, on);
            if on {
                swatch = swatch.child(motion::once(
                    icon("check").size(px(16.0)).text_color(if c.is_some() { rgb(0xffffff) } else { p.foreground }),
                    SharedString::from(format!("swatch-check-{c:?}")),
                    Duration::from_millis(260),
                    |el, t| el.opacity(t),
                ));
            } else if c.is_none() {
                swatch = swatch.child(icon("slash").size(px(20.0)).text_color(alpha(p.muted_foreground, 0.6)));
            }
            swatches = swatches.child(swatch);
        }
        // Any color at all: the rainbow, or the color itself once it's one of a kind.
        let custom = color.is_some_and(|c| !SWATCHES.contains(&c));
        let rainbow = div()
            .id("swatch-custom")
            .size(px(36.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .border_2()
            .border_dashed()
            .map(|el| match color.filter(|_| custom) {
                Some(c) => el.border_color(gpui_kit::transparent_black()).bg(color_of(c)),
                None => el.border_color(alpha(p.muted_foreground, 0.4)).bg(gpui_kit::linear_gradient(
                    135.0,
                    gpui_kit::linear_color_stop(color_of(0xf472b6), 0.0),
                    gpui_kit::linear_color_stop(color_of(0x38bdf8), 1.0),
                )),
            })
            .when(locked, |el| el.opacity(0.5))
            .when(!locked, |el| {
                el.cursor_pointer().on_click(cx.listener(|this, _, window, cx| {
                    this.roles.custom = !this.roles.custom;
                    if this.roles.custom {
                        this.roles.hex.update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                }))
            })
            .when(custom, |el| el.child(icon("check").size(px(16.0)).text_color(rgb(0xffffff))));
        swatches = swatches.child(ring(rainbow, custom));
        let hex_box = self.roles.custom.then(|| {
            super::pages::slide_in(
                div().mt(px(8.0)).w(px(160.0)).child(super::pages::boxed(
                    Input::new(&self.roles.hex).appearance(false),
                    36.0,
                    super::pages::focused(&self.roles.hex, window, cx),
                    p,
                )),
                "role-hex",
            )
        });
        let hoist = e.hoist.unwrap_or(role.hoist);
        let mentionable = e.mentionable.unwrap_or(role.mentionable);
        let mut toggle = |id: &'static str, title: &str, body: &str, on: bool, set: fn(&mut Edits, bool)| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .when(locked, |el| el.opacity(0.6))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div().font_weight(FontWeight::BOLD).text_sm().line_height(px(20.0)).child(title.to_owned()),
                        )
                        .child(
                            div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(body.to_owned()),
                        ),
                )
                .child(crate::ui::settings_controls::switch(
                    id,
                    on,
                    locked,
                    p,
                    window,
                    cx,
                    move |this: &mut Self, v, cx| {
                        set(&mut this.roles.edits, v);
                        cx.notify();
                    },
                ))
        };
        let hoist_toggle = toggle(
            "role-hoist",
            &t("serversettings.roles.hoist"),
            &t("serversettings.roles.hoistHint"),
            hoist,
            |e, v| e.hoist = Some(v),
        );
        let mention_toggle = toggle(
            "role-mentionable",
            &t("serversettings.roles.mentionable"),
            &t("serversettings.roles.mentionableHint"),
            mentionable,
            |e, v| e.mentionable = Some(v),
        );
        let mention_bg = color.map(|c| color_of(c).opacity(0.15)).unwrap_or(alpha(p.primary, 0.15));
        let mention_fg = color.map(color_of).unwrap_or(p.primary.into());
        let me = snap.me.clone();
        let my_name = me.as_ref().map(user_name).unwrap_or_else(|| t("serversettings.shared.you"));
        let my_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
        // The greeting around the mention, split where the role goes.
        let line = t_with("serversettings.roles.previewLine", &[("role", Arg::Str("\u{E000}"))]);
        let (hey, welcome) = line.split_once('\u{E000}').unwrap_or((line.as_str(), ""));
        let (hey, welcome) = (hey.to_owned(), welcome.to_owned());
        let preview = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(radius_2xl())
            .bg(alpha(p.muted, 0.5))
            .child(
                div().flex().items_center().gap(px(10.0)).child(avatar(me.as_ref(), 36.0, p)).child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_color(match color {
                                    Some(c) => color_of(c),
                                    None => crate::ui::widgets::name_tint(&my_id, p),
                                })
                                .child(my_name),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .child(hey.replace(' ', "\u{a0}"))
                                .child(
                                    div()
                                        .px(px(4.0))
                                        .rounded(radius_md())
                                        .bg(mention_bg)
                                        .text_color(mention_fg)
                                        .font_weight(FontWeight::BOLD)
                                        .child(format!("@{shown}")),
                                )
                                .child(welcome.replace(' ', "\u{a0}")),
                        ),
                ),
            )
            .when(hoist, |el| {
                el.child(super::pages::slide_in(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(dot(color, 8.0, p))
                        .child(format!("{} — 1", shown.to_uppercase())),
                    "role-hoist-preview",
                ))
            });

        let delete = if locked {
            None
        } else if self.roles.confirming {
            let id = role.id.clone();
            let busy = self.roles.saving;
            Some(
                motion::rise(
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
                                .child(t_with("serversettings.roles.deleteAsk", &[("role", Arg::Str(&role.name))])),
                        )
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(20.0))
                                .text_color(p.muted_foreground)
                                .child(t("serversettings.roles.deleteHint")),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    button("role-keep", t("serversettings.shared.keepIt"), None, Look::Ghost, false, p)
                                        .rounded(radius_xl())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.roles.confirming = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    button(
                                        "role-delete-yes",
                                        t("serversettings.shared.delete"),
                                        if busy { None } else { Some("trash") },
                                        Look::Destructive,
                                        false,
                                        p,
                                    )
                                    .rounded(radius_xl())
                                    .font_weight(FontWeight::BOLD)
                                    .when(busy, |el| el.opacity(0.6))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if !this.roles.saving {
                                                this.delete_role(id.clone(), cx)
                                            }
                                        },
                                    )),
                                ),
                        ),
                    "role-confirm",
                    Duration::ZERO,
                    8.0,
                )
                .into_any_element(),
            )
        } else {
            let red = alpha(p.destructive, 0.1);
            let destructive = p.destructive;
            Some(
                div()
                    .flex()
                    .child(
                        super::pages::hover_button(
                            "role-delete",
                            t("serversettings.roles.delete"),
                            Some("trash"),
                            false,
                            p,
                            move |s| s.bg(red),
                        )
                        .text_color(destructive)
                        .font_weight(FontWeight::MEDIUM)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.roles.confirming = true;
                            cx.notify();
                        })),
                    )
                    .into_any_element(),
            )
        };

        div()
            .flex()
            .flex_col()
            .child(
                form_row(
                    &t("serversettings.roles.name"),
                    None,
                    super::pages::boxed(
                        Input::new(&self.roles.name).appearance(false).disabled(locked),
                        44.0,
                        super::pages::focused(&self.roles.name, window, cx),
                        p,
                    ),
                    false,
                    p,
                )
                // The first row sits right under the tabs, as the web's does.
                .pt(px(0.0)),
            )
            .child(form_row(
                &t("serversettings.roles.color"),
                Some(t("serversettings.roles.colorHint")),
                div().flex().flex_col().child(swatches).children(hex_box),
                false,
                p,
            ))
            .child(form_row(
                &t("serversettings.roles.howItShows"),
                None,
                div().flex().flex_col().gap(px(16.0)).child(hoist_toggle).child(mention_toggle),
                false,
                p,
            ))
            .child(form_row(&t("settings.controls.preview"), None, preview, true, p))
            .children(delete.map(|d| div().py(px(20.0)).child(d)))
    }

    #[allow(clippy::too_many_arguments)]
    fn role_permissions(
        &mut self,
        role: &pb::Role,
        e: &Edits,
        everyone: bool,
        locked: bool,
        access: &Access,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let bits = e.permissions(role);
        let live = permissions::from_list(&role.permissions);
        let query = self.roles.perm_query.read(cx).value().trim().to_lowercase();
        let mut out = div().flex().flex_col().gap(px(16.0)).child(
            div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(if everyone {
                t("serversettings.roles.everyoneIntro")
            } else {
                t("serversettings.roles.roleIntro")
            }),
        );
        // Clearing takes away only what you could give.
        let clearable = permissions::KNOWN
            .iter()
            .filter(|p| bits & bit(**p) != 0 && access.may_change(bit(**p), access.server))
            .fold(0, |b, p| b | bit(*p));
        out = out.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(super::people::search_box(&self.roles.perm_query, p, window, cx).flex_1().min_w(px(192.0)))
                .child(
                    button("role-clear", t("serversettings.roles.clearAll"), None, Look::Ghost, true, p)
                        .rounded(radius_xl())
                        .when(locked || clearable == 0, |el| el.opacity(0.5))
                        .when(!locked && clearable != 0, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.roles.edits.switch(live, clearable, false);
                                cx.notify();
                            }))
                        }),
                ),
        );
        let mut shown = 0usize;
        for (g, (title, list)) in GROUPS.iter().enumerate() {
            let matching: Vec<P> = list
                .iter()
                .copied()
                .filter(|perm| {
                    query.is_empty()
                        || format!("{} {}", permission_name(*perm), permission_about(*perm))
                            .to_lowercase()
                            .contains(&query)
                })
                .collect();
            if matching.is_empty() {
                continue;
            }
            let mut section = div().flex().flex_col().child(
                div()
                    .mb(px(4.0))
                    .text_size(px(11.2))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(group_name(title).to_uppercase()),
            );
            let count = matching.len();
            for (n, perm) in matching.into_iter().enumerate() {
                let (label, about) = (permission_name(perm), permission_about(perm));
                let on = bits & bit(perm) != 0;
                let allowed = access.may_change(bit(perm), access.server);
                let admin = perm == P::Administrator;
                let last = n + 1 == count;
                let row = div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .py(px(12.0))
                    .when(!last, |el| el.border_b_1().border_color(alpha(p.border, 0.6)))
                    .when(admin && on, |el| {
                        el.mx(px(-12.0))
                            .px(px(12.0))
                            .rounded(radius_xl())
                            .border_color(gpui_kit::transparent_black())
                            .bg(alpha(p.destructive, 0.08))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::BOLD)
                                    .when(admin, |el| {
                                        el.child(icon("shield-alert").size(px(16.0)).text_color(if on {
                                            p.destructive
                                        } else {
                                            p.muted_foreground
                                        }))
                                    })
                                    .child(label)
                                    .when(!allowed && !locked, |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .px(px(8.0))
                                                .py(px(2.0))
                                                .rounded_full()
                                                .bg(p.muted)
                                                .text_size(px(10.4))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(p.muted_foreground)
                                                .child(icon("lock").size(px(12.0)))
                                                .child(t("serversettings.roles.notYours")),
                                        )
                                    }),
                            )
                            .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(about)),
                    )
                    .child(crate::ui::settings_controls::switch(
                        SharedString::from(format!("perm-{}", perm as i32)),
                        on,
                        locked || !allowed,
                        p,
                        window,
                        cx,
                        move |this: &mut Self, v, cx| {
                            this.roles.edits.switch(live, bit(perm), v);
                            cx.notify();
                        },
                    ));
                section = section.child(motion::rise(
                    row,
                    SharedString::from(format!("perm-row-{}", perm as i32)),
                    Duration::from_millis(15 * (g * 4 + n).min(16) as u64),
                    6.0,
                ));
                shown += 1;
            }
            out = out.child(section);
        }
        if shown == 0 {
            out = out.child(
                div()
                    .py(px(24.0))
                    .text_center()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.roles.noMatch")),
            );
        }
        out
    }

    fn role_members(
        &mut self,
        role: &pb::Role,
        locked: bool,
        snap: &Snap,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let query = self.roles.member_query.read(cx).value().trim().to_lowercase();
        let busy = self.roles.busy.clone();
        let color = role_color(role);
        let mut out = div().flex().flex_col().gap(px(12.0));
        if !locked {
            let adding = self.roles.adding;
            out = out.child(
                div().flex().child(
                    button(
                        "role-add-members",
                        t("serversettings.roles.addMembers"),
                        Some("user-plus"),
                        if adding { Look::Ghost } else { Look::Outline },
                        false,
                        p,
                    )
                    .rounded(radius_xl())
                    .font_weight(FontWeight::BOLD)
                    .when(adding, |el| el.bg(p.secondary))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.roles.adding = !this.roles.adding;
                        if this.roles.adding {
                            this.roles.member_query.update(cx, |s, cx| s.focus(window, cx));
                        }
                        cx.notify();
                    })),
                ),
            );
            if adding {
                let others: Vec<&pb::Member> = snap
                    .members
                    .iter()
                    .filter(|m| !m.role_ids.contains(&role.id))
                    .filter(|m| {
                        query.is_empty()
                            || format!(
                                "{} {}",
                                member_name(m),
                                m.user.as_ref().map(|u| u.username.as_str()).unwrap_or("")
                            )
                            .to_lowercase()
                            .contains(&query)
                    })
                    .take(30)
                    .collect();
                let mut list =
                    div().id("role-others").flex().flex_col().gap(px(2.0)).max_h(px(240.0)).overflow_y_scroll();
                if others.is_empty() {
                    list = list.child(
                        div().px(px(8.0)).py(px(12.0)).text_center().text_sm().text_color(p.muted_foreground).child(
                            if query.is_empty() {
                                t("serversettings.roles.everyoneHas")
                            } else {
                                t("serversettings.shared.nobodyMatches")
                            },
                        ),
                    );
                }
                for (n, m) in others.into_iter().enumerate() {
                    let Some(user) = m.user.clone() else { continue };
                    let (uid, rid) = (user.id.clone(), role.id.clone());
                    let hover = alpha(p.primary, 0.1);
                    let group = SharedString::from(format!("role-give-g-{}", user.id));
                    list = list.child(motion::slide_in(
                        div()
                            .id(SharedString::from(format!("role-give-{}", user.id)))
                            .group(group.clone())
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(radius_xl())
                            .cursor_pointer()
                            .text_sm()
                            .hover(move |s| s.bg(hover))
                            .when(busy.as_deref() == Some(user.id.as_str()), |el| el.opacity(0.5))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.give_role(uid.clone(), rid.clone(), true, cx)),
                            )
                            .child(avatar(Some(&user), 28.0, p))
                            .child(
                                div().flex_1().min_w_0().truncate().font_weight(FontWeight::BOLD).child(member_name(m)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(format!("@{}", user.username)),
                            )
                            .child(
                                div()
                                    .opacity(0.0)
                                    .group_hover(group, |s| s.opacity(1.0))
                                    .text_color(p.primary)
                                    .child(icon("plus").size(px(16.0))),
                            ),
                        SharedString::from(format!("role-other-{}-{n}", user.id)),
                        -8.0,
                    ));
                }
                out = out.child(super::pages::slide_in(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .p(px(8.0))
                        .rounded(radius_2xl())
                        .border_1()
                        .border_color(p.border)
                        .bg(alpha(p.background, 0.4))
                        .child(super::people::search_box(&self.roles.member_query, p, window, cx))
                        .child(list),
                    "role-adding",
                ));
            }
        }
        let holders: Vec<&pb::Member> = snap.members.iter().filter(|m| m.role_ids.contains(&role.id)).collect();
        if holders.is_empty() {
            out = out.child(
                div()
                    .py(px(24.0))
                    .text_center()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.roles.nobody")),
            );
        }
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (n, m) in holders.into_iter().enumerate() {
            let Some(user) = m.user.clone() else { continue };
            let (uid, rid) = (user.id.clone(), role.id.clone());
            let red = alpha(p.destructive, 0.1);
            let hover_border = alpha(p.primary, 0.3);
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(format!("role-holder-row-{}", user.id)))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(8.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.4))
                    .hover(move |s| s.border_color(hover_border))
                    .when(busy.as_deref() == Some(user.id.as_str()), |el| el.opacity(0.5))
                    .child(avatar(Some(&user), 36.0, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(match color {
                                        Some(c) => color_of(c),
                                        None => crate::ui::widgets::name_tint(&user.id, p),
                                    })
                                    .child(member_name(m)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(format!("@{}", user.username)),
                            ),
                    )
                    .when(!locked, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("role-take-{}", user.id)))
                                .size(px(36.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(radius_xl())
                                .cursor_pointer()
                                .text_color(p.muted_foreground)
                                .hover({
                                    let fg = p.destructive;
                                    move |s| s.bg(red).text_color(fg)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.give_role(uid.clone(), rid.clone(), false, cx)
                                }))
                                .child(icon("user-minus").size(px(16.0))),
                        )
                    }),
                SharedString::from(format!("role-holder-{}", user.id)),
                Duration::from_millis(20 * n.min(12) as u64),
                10.0,
            ));
        }
        out.child(list)
    }
}

/// A role being dragged into rank, and the copy of it that follows the pointer.
#[derive(Clone)]
pub(super) struct RoleDrag {
    id: String,
    name: SharedString,
    color: Option<u32>,
}

impl Render for RoleDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        div()
            .w(px(220.0))
            .h(px(36.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_lg())
            .bg(p.background)
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .text_color(p.foreground)
            .shadow(vec![gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.45),
                offset: point(px(0.0), px(12.0)),
                blur_radius: px(30.0),
                spread_radius: px(-12.0),
                inset: false,
            }])
            .child(icon("grip-vertical").size(px(16.0)).text_color(p.muted_foreground))
            .child(dot(self.color, 12.0, &p))
            .child(self.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::Permission as P;

    #[test]
    fn a_role_saved_elsewhere_while_open_keeps_both_changes() {
        // Opened with Manage Messages: here Ban Members goes on and Manage Messages off...
        let opened = bit(P::ManageMessages);
        let mut edits = Edits::default();
        edits.switch(opened, bit(P::BanMembers), true);
        edits.switch(opened, bit(P::ManageMessages), false);
        // ...while someone else saves Kick Members.
        let now = pb::Role { permissions: vec![P::ManageMessages as i32, P::KickMembers as i32], ..Default::default() };
        assert_eq!(edits.permissions(&now), bit(P::KickMembers) | bit(P::BanMembers));
        assert_eq!(edits.changes(&now), (bit(P::BanMembers), bit(P::ManageMessages)));
    }

    #[test]
    fn a_switch_flipped_and_put_back_leaves_someone_elses_change() {
        // Ban Members goes on and off again here, then someone else grants it.
        let opened = bit(P::ManageMessages);
        let mut edits = Edits::default();
        edits.switch(opened, bit(P::BanMembers), true);
        edits.switch(opened, bit(P::BanMembers), false);
        let now = pb::Role { permissions: vec![P::ManageMessages as i32, P::BanMembers as i32], ..Default::default() };
        assert_eq!(edits.changes(&now), (0, 0));
    }
}
