//! The Roles page: every role, highest first, moved up or down past the ones
//! ranked below your own, and the chosen one beside the list: how it looks,
//! what it can do and who has it. @everyone sits at the bottom and holds what
//! everybody can do. The web's `settings/server/Roles.tsx`.

use std::rc::Rc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::rgb;

use super::*;
use crate::core::permissions::{self, Access, Bits, GROUPS, bit};
use crate::core::server_admin::RolePatch;

/// Colors to pick from, light to deep, as on the web.
const SWATCHES: [u32; 24] = [
    0xf472b6, 0xfb7185, 0xf87171, 0xfb923c, 0xfbbf24, 0xa3e635, 0x34d399, 0x2dd4bf, 0x38bdf8, 0x60a5fa, 0x818cf8,
    0xa78bfa, 0xe879f9, 0xdb2777, 0xe11d48, 0xea580c, 0xca8a04, 0x16a34a, 0x0d9488, 0x0284c7, 0x4f46e5, 0x7c3aed,
    0x94a3b8, 0x64748b,
];

/// How tall a row in the role list is, gap included.
const ROW: f32 = 42.0;

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
    permissions: Option<Bits>,
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
}

impl Roles {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("new role"));
        let perm_query = cx.new(|cx| InputState::new(window, cx).placeholder("Search permissions"));
        let member_query = cx.new(|cx| InputState::new(window, cx).placeholder("Find someone"));
        let subscriptions = [&name, &perm_query, &member_query]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                    if let InputEvent::Change = e {
                        cx.notify()
                    }
                })
            })
            .collect();
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

fn role_color(r: &pb::Role) -> Option<u32> {
    r.color.map(|c| c as u32)
}

fn member_name(m: &pb::Member) -> String {
    match &m.user {
        Some(u) if m.nickname.is_empty() => user_name(u),
        Some(_) => m.nickname.clone(),
        None => String::new(),
    }
}

/// A small dot in a role's color, or an outline for one without.
fn dot(color: Option<u32>, size: f32, p: &Palette) -> gpui_kit::Div {
    let d = div().flex_none().size(px(size)).rounded_full();
    match color {
        Some(c) => d.bg(color_of(c)),
        None => d.border_2().border_color(alpha(p.muted_foreground, 0.6)),
    }
}

/// A switch that calls back into the page.
pub(super) fn switch(
    id: SharedString,
    on: bool,
    disabled: bool,
    cx: &mut Context<ServerSettingsView>,
    set: impl Fn(&mut ServerSettingsView, bool, &mut Context<ServerSettingsView>) + 'static,
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
        self.run(cx, async move { core.create_role(&key, &sid, "new role").await }, |this, result, cx| {
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

    /// Swaps a role with the one above or below it, when you rank above both.
    fn move_role(&mut self, order: &[String], id: &str, by: isize, cx: &mut Context<Self>) {
        let Some(at) = order.iter().position(|r| r == id) else { return };
        let to = at as isize + by;
        if to < 0 || to as usize >= order.len() || self.roles.moving.is_some() {
            return;
        }
        let mut next = order.to_vec();
        next.swap(at, to as usize);
        self.roles.moving = Some(next.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.reorder_roles(&key, &sid, next).await }, |this, result, cx| {
            this.roles.moving = None;
            if let Err(err) = result {
                this.error = Some(err.message);
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
            e.permissions.is_some_and(|b| b != permissions::from_list(&role.permissions)),
        ]
        .into_iter()
        .filter(|c| *c)
        .count()
    }

    fn save_role(&mut self, role: &pb::Role, everyone: bool, cx: &mut Context<Self>) {
        let e = self.roles.edits.clone();
        let name = self.roles.name.read(cx).value().trim().to_string();
        if !everyone && name.is_empty() {
            self.error = Some("A role needs a name.".into());
            cx.notify();
            return;
        }
        let patch = RolePatch {
            name: (!everyone && name != role.name).then_some(name),
            color: e.color.filter(|c| *c != role_color(role)),
            hoist: e.hoist.filter(|h| *h != role.hoist),
            mentionable: e.mentionable.filter(|m| *m != role.mentionable),
            permissions: e
                .permissions
                .filter(|b| *b != permissions::from_list(&role.permissions))
                .map(permissions::to_list),
        };
        self.roles.saving = true;
        self.error = None;
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
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn discard_role(&mut self, role: &pb::Role, window: &mut Window, cx: &mut Context<Self>) {
        self.roles.edits = Edits::default();
        self.roles.filled_for = None;
        self.error = None;
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
        if everyone && self.roles.tab != RoleTab::Permissions {
            self.roles.tab = RoleTab::Permissions;
        }
        if let Some(role) = &role {
            self.fill_role(role, window, cx);
        }

        // The list, each row sliding to its place when the order changes.
        let mut rows = div().relative().h(px(ROW * order.len() as f32));
        for (n, id) in order.iter().enumerate() {
            let Some(r) = by_id.get(id.as_str()) else { continue };
            let y = motion::follow(SharedString::from(format!("role-y-{id}")), n as f32 * ROW, window, cx);
            let active = *id == selected;
            let locked = !snap.access.above(r.position);
            // You can't lift a role past one you don't rank above.
            let up =
                !locked && n > 0 && by_id.get(order[n - 1].as_str()).is_some_and(|o| snap.access.above(o.position));
            let down = !locked && n + 1 < order.len();
            let row = self.role_row(
                r,
                counts.get(id.as_str()).copied().unwrap_or(0),
                active,
                locked,
                up,
                down,
                &order,
                p,
                cx,
            );
            rows = rows.child(div().absolute().left_0().right_0().top(px(y)).child(motion::rise(
                row,
                SharedString::from(format!("role-in-{id}")),
                Duration::from_millis(25 * n.min(12) as u64),
                6.0,
            )));
        }
        let everyone_row = {
            let on = everyone;
            let hover = alpha(p.muted, 0.7);
            div()
                .id("role-everyone")
                .mt(px(8.0))
                .h(px(48.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(corner(12.0))
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .cursor_pointer()
                .when(on, |el| el.bg(alpha(p.primary, 0.12)).text_color(p.primary))
                .when(!on, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover)))
                .on_click({
                    let id = everyone_id.clone();
                    cx.listener(move |this, _, _, cx| this.select_role(id.clone(), cx))
                })
                .child(icon("users").size(px(16.0)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_weight(FontWeight::BOLD).child("@everyone"))
                        .child(div().text_xs().text_color(p.muted_foreground).child("What everybody can do")),
                )
        };
        let can_create = snap.access.has(P::ManageRoles);
        let making = self.roles.busy.as_deref() == Some("new");
        let list = div()
            .w(px(260.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child("Higher roles outrank lower ones. Move them with the arrows."),
                    )
                    .when(can_create, |el| {
                        el.child(
                            primary_button("role-new", if making { "Making…" } else { "New" }, p)
                                .h(px(34.0))
                                .px(px(12.0))
                                .child(icon("plus").size(px(15.0)))
                                .when(making, |el| el.opacity(0.6))
                                .on_click(cx.listener(|this, _, _, cx| this.new_role(cx))),
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
                    .child(rows)
                    .child(everyone_row),
            );

        let editor = match role {
            Some(role) => motion::rise(
                self.role_editor(&role, everyone, &snap, p, window, cx),
                SharedString::from(format!("role-editor-{}", role.id)),
                Duration::ZERO,
                10.0,
            )
            .into_any_element(),
            None => div()
                .py(px(40.0))
                .text_center()
                .text_sm()
                .text_color(p.muted_foreground)
                .child("Pick a role to change it.")
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
    fn role_row(
        &self,
        r: &pb::Role,
        count: usize,
        active: bool,
        locked: bool,
        up: bool,
        down: bool,
        order: &[String],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let color = role_color(r);
        let hover = alpha(p.muted, 0.7);
        let mover = |glyph: &str, by: isize, enabled: bool, cx: &mut Context<Self>| {
            let (id, order) = (r.id.clone(), order.to_vec());
            let hover = alpha(p.muted_foreground, 0.14);
            div()
                .id(SharedString::from(format!("role-{glyph}-{}", r.id)))
                .size(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(5.0))
                .text_color(alpha(p.muted_foreground, if enabled { 0.9 } else { 0.25 }))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.move_role(&order, &id, by, cx)))
                })
                .child(icon(glyph).size(px(14.0)))
        };
        let handle = if locked {
            div()
                .w(px(20.0))
                .flex()
                .justify_center()
                .text_color(alpha(p.muted_foreground, 0.5))
                .child(icon("lock").size(px(13.0)))
        } else {
            div().w(px(20.0)).flex().flex_col().items_center().child(mover("chevron-up", -1, up, cx)).child(mover(
                "chevron-down",
                1,
                down,
                cx,
            ))
        };
        let id = r.id.clone();
        div().h(px(ROW - 4.0)).flex().items_center().gap(px(2.0)).child(handle).child(
            div()
                .id(SharedString::from(format!("role-pick-{}", r.id)))
                .flex_1()
                .min_w_0()
                .h_full()
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(corner(10.0))
                .cursor_pointer()
                .text_sm()
                .when(active, |el| el.bg(alpha(p.primary, 0.12)).font_weight(FontWeight::BOLD))
                .when(!active, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.select_role(id.clone(), cx)))
                .child(dot(color, 10.0, p))
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
                        .gap(px(3.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(icon("users").size(px(11.0)))
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
        let shown = if typed.is_empty() { "new role".to_owned() } else { typed };
        let holders = snap.members.iter().filter(|m| m.role_ids.contains(&role.id)).count();

        let mut tabs = div().flex().gap(px(4.0)).p(px(4.0)).rounded(corner(12.0)).bg(alpha(p.muted, 0.6));
        let options: Vec<(RoleTab, String)> = if everyone {
            vec![(RoleTab::Permissions, "Permissions".into())]
        } else {
            vec![
                (RoleTab::Display, "Display".into()),
                (RoleTab::Permissions, "Permissions".into()),
                (RoleTab::Members, format!("Members ({holders})")),
            ]
        };
        if options.len() > 1 {
            for (tab, label) in options {
                let on = self.roles.tab == tab;
                let hover = alpha(p.foreground, 0.06);
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!("role-tab-{label}")))
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
                            this.roles.tab = tab;
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
                    .when(!everyone, |el| {
                        el.child(motion::rise(
                            dot(color, 14.0, p),
                            SharedString::from(format!("role-dot-{color:?}")),
                            Duration::ZERO,
                            -6.0,
                        ))
                    })
                    .child(
                        div()
                            .truncate()
                            .when_some(color.filter(|_| !everyone), |el, c| el.text_color(color_of(c)))
                            .child(if everyone { "@everyone".to_owned() } else { shown.clone() }),
                    ),
            )
            .child(tabs);

        let tab = self.roles.tab;
        let body = match tab {
            RoleTab::Display if !everyone => self.role_display(role, &e, color, &shown, locked, snap, p, cx),
            RoleTab::Members if !everyone => self.role_members(role, locked, snap, p, cx),
            _ => self.role_permissions(role, &e, everyone, locked, &snap.access, p, cx),
        };

        let n = self.changes(role, everyone, cx);
        let saving = self.roles.saving;
        if n > 0 {
            let (r1, r2) = (role.clone(), role.clone());
            self.bar = Some(save_bar(
                "role-save-bar",
                n,
                saving,
                p,
                cx,
                move |this, window, cx| this.discard_role(&r1, window, cx),
                move |this, cx| {
                    if !this.roles.saving {
                        this.save_role(&r2, everyone, cx)
                    }
                },
            ));
        }
        let _ = window;
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(locked, |el| {
                el.child(
                    div()
                        .mb(px(14.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(corner(12.0))
                        .bg(alpha(p.muted, 0.6))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(icon("lock").size(px(15.0)))
                        .child("This role ranks at or above your highest role, so you can't change it."),
                )
            })
            .child(motion::rise(
                body,
                SharedString::from(format!("role-body-{}-{}", role.id, tab as u8)),
                Duration::ZERO,
                8.0,
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
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let mut swatches = div().flex().flex_wrap().gap(px(8.0));
        for c in std::iter::once(None).chain(SWATCHES.iter().map(|c| Some(*c))) {
            let on = color == c;
            let ring = p.foreground;
            let mut swatch = div()
                .id(SharedString::from(format!("swatch-{c:?}")))
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .when(locked, |el| el.opacity(0.5))
                .when(!locked, |el| {
                    el.cursor_pointer().hover(|s| s.size(px(34.0)).m(px(-1.0))).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.roles.edits.color = Some(c);
                            cx.notify();
                        },
                    ))
                });
            swatch = match c {
                Some(c) => swatch.bg(color_of(c)),
                None => swatch.border_2().border_color(alpha(p.muted_foreground, 0.4)).bg(p.background),
            };
            if on {
                swatch = swatch.border_2().border_color(ring).child(motion::rise(
                    icon("check").size(px(16.0)).text_color(if c.is_some() { rgb(0xffffff) } else { p.foreground }),
                    SharedString::from(format!("swatch-check-{c:?}")),
                    Duration::ZERO,
                    -4.0,
                ));
            } else if c.is_none() {
                swatch = swatch.child(div().w(px(16.0)).h(px(2.0)).rounded_full().bg(alpha(p.muted_foreground, 0.6)));
            }
            swatches = swatches.child(swatch);
        }
        let hoist = e.hoist.unwrap_or(role.hoist);
        let mentionable = e.mentionable.unwrap_or(role.mentionable);
        let toggle =
            |id: &str, title: &str, body: &str, on: bool, cx: &mut Context<Self>, set: fn(&mut Edits, bool)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::BOLD).text_sm().child(title.to_owned()))
                            .child(div().text_xs().text_color(p.muted_foreground).child(body.to_owned())),
                    )
                    .child(switch(SharedString::from(id.to_owned()), on, locked, cx, move |this, v, cx| {
                        set(&mut this.roles.edits, v);
                        cx.notify();
                    }))
            };
        let mention_bg = color.map(|c| color_of(c).opacity(0.15)).unwrap_or(alpha(p.primary, 0.15));
        let mention_fg = color.map(color_of).unwrap_or(p.primary.into());
        let me = snap.me.clone();
        let my_name = me.as_ref().map(user_name).unwrap_or_else(|| "You".into());
        let preview = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .bg(alpha(p.muted, 0.5))
            .child(
                div().flex().items_center().gap(px(10.0)).child(avatar(me.as_ref(), 36.0, p)).child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .when_some(color, |el, c| el.text_color(color_of(c)))
                                .child(my_name),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(0.0))
                                .text_sm()
                                .text_color(p.muted_foreground)
                                .child("Hey")
                                .child(
                                    div()
                                        .ml(px(4.0))
                                        .px(px(4.0))
                                        .rounded(corner(6.0))
                                        .bg(mention_bg)
                                        .text_color(mention_fg)
                                        .font_weight(FontWeight::BOLD)
                                        .child(format!("@{shown}")),
                                )
                                .child(", welcome aboard!"),
                        ),
                ),
            )
            .when(hoist, |el| {
                el.child(motion::rise(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(dot(color, 8.0, p))
                        .child(format!("{} — 1", shown.to_uppercase())),
                    "role-hoist-preview",
                    Duration::ZERO,
                    -6.0,
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
                        .gap(px(10.0))
                        .p(px(16.0))
                        .rounded(corner(16.0))
                        .border_1()
                        .border_color(alpha(p.destructive, 0.4))
                        .bg(alpha(p.destructive, 0.06))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.destructive)
                                .child(icon("triangle-alert").size(px(16.0)))
                                .child(format!("Delete {}?", role.name)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.muted_foreground)
                                .child("Everyone who has it loses it, and every channel forgets what it allowed."),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(soft_button("role-keep", "Keep it", p).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.roles.confirming = false;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    danger_button("role-delete-yes", if busy { "Deleting…" } else { "Delete" }, p)
                                        .child(icon("trash").size(px(15.0)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.roles.saving {
                                                this.delete_role(id.clone(), cx)
                                            }
                                        })),
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
            Some(
                div()
                    .child(
                        div()
                            .id("role-delete")
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(12.0))
                            .h(px(36.0))
                            .rounded(corner(12.0))
                            .cursor_pointer()
                            .text_color(p.destructive)
                            .font_weight(FontWeight::BOLD)
                            .text_sm()
                            .hover(move |s| s.bg(red))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.roles.confirming = true;
                                cx.notify();
                            }))
                            .child(icon("trash").size(px(15.0)))
                            .child("Delete role"),
                    )
                    .into_any_element(),
            )
        };

        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(labeled("Role name", Input::new(&self.roles.name).large().disabled(locked), p))
            .child(labeled(
                "Color",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child("Members take the color of their highest role that has one."),
                    )
                    .child(swatches),
                p,
            ))
            .child(labeled(
                "How it shows",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(toggle(
                        "role-hoist",
                        "List members apart",
                        "People with this role get their own group in the member list.",
                        hoist,
                        cx,
                        |e, v| e.hoist = Some(v),
                    ))
                    .child(toggle(
                        "role-mentionable",
                        "Let anyone @mention this role",
                        "People who can mention everyone can always mention it.",
                        mentionable,
                        cx,
                        |e, v| e.mentionable = Some(v),
                    )),
                p,
            ))
            .child(labeled("Preview", preview, p))
            .children(delete)
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
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let bits = e.permissions.unwrap_or_else(|| permissions::from_list(&role.permissions));
        let query = self.roles.perm_query.read(cx).value().trim().to_lowercase();
        let base = bits;
        let mut out = div().flex().flex_col().gap(px(16.0)).child(
            div().text_sm().text_color(p.muted_foreground).child(if everyone {
                "What every member can do, before their roles add more. Channels can still say otherwise."
            } else {
                "What people with this role can do, on top of @everyone. Channels can still say otherwise."
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
                .items_center()
                .gap(px(8.0))
                .child(div().flex_1().child(Input::new(&self.roles.perm_query).prefix(icon("search").size(px(16.0)))))
                .child(
                    soft_button("role-clear", "Clear all", p)
                        .when(locked || clearable == 0, |el| el.opacity(0.5))
                        .when(!locked && clearable != 0, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.roles.edits.permissions = Some(base & !clearable);
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
                    let (label, about) = permissions::info(*perm);
                    query.is_empty() || format!("{label} {about}").to_lowercase().contains(&query)
                })
                .collect();
            if matching.is_empty() {
                continue;
            }
            let mut section = div().flex().flex_col().child(
                div()
                    .mb(px(4.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(title.to_uppercase()),
            );
            for (n, perm) in matching.into_iter().enumerate() {
                let (label, about) = permissions::info(perm);
                let on = bits & bit(perm) != 0;
                let allowed = access.may_change(bit(perm), access.server);
                let admin = perm == P::Administrator;
                let row = div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(alpha(p.border, 0.6))
                    .when(admin && on, |el| {
                        el.px(px(12.0))
                            .rounded(corner(12.0))
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
                                    .font_weight(FontWeight::BOLD)
                                    .when(admin, |el| {
                                        el.child(icon("shield-alert").size(px(15.0)).text_color(if on {
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
                                                .px(px(7.0))
                                                .h(px(18.0))
                                                .rounded_full()
                                                .bg(p.muted)
                                                .text_size(px(10.0))
                                                .text_color(p.muted_foreground)
                                                .child(icon("lock").size(px(10.0)))
                                                .child("Not yours to give"),
                                        )
                                    }),
                            )
                            .child(div().text_xs().text_color(p.muted_foreground).child(about)),
                    )
                    .child(switch(
                        SharedString::from(format!("perm-{}", perm as i32)),
                        on,
                        locked || !allowed,
                        cx,
                        move |this, v, cx| {
                            let now = this.roles.edits.permissions.unwrap_or(base);
                            this.roles.edits.permissions = Some(if v { now | bit(perm) } else { now & !bit(perm) });
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
                    .child("No permission matches that."),
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
                    soft_button("role-add-members", "Add members", p)
                        .child(icon("user-plus").size(px(15.0)))
                        .when(adding, |el| el.bg(alpha(p.primary, 0.16)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.roles.adding = !this.roles.adding;
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
                    list =
                        list.child(
                            div().py(px(12.0)).text_center().text_sm().text_color(p.muted_foreground).child(
                                if query.is_empty() { "Everyone has this role." } else { "Nobody matches that." },
                            ),
                        );
                }
                for (n, m) in others.into_iter().enumerate() {
                    let Some(user) = m.user.clone() else { continue };
                    let (uid, rid) = (user.id.clone(), role.id.clone());
                    let hover = alpha(p.primary, 0.1);
                    list = list.child(motion::rise(
                        div()
                            .id(SharedString::from(format!("role-give-{}", user.id)))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(corner(12.0))
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
                            .child(div().text_xs().text_color(p.muted_foreground).child(format!("@{}", user.username)))
                            .child(icon("plus").size(px(15.0)).text_color(p.primary)),
                        SharedString::from(format!("role-other-{}", user.id)),
                        Duration::from_millis(15 * n.min(12) as u64),
                        -6.0,
                    ));
                }
                out = out.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .p(px(8.0))
                        .rounded(corner(16.0))
                        .border_1()
                        .border_color(p.border)
                        .bg(alpha(p.background, 0.4))
                        .child(Input::new(&self.roles.member_query).prefix(icon("search").size(px(16.0))))
                        .child(list),
                    "role-adding",
                    Duration::ZERO,
                    -8.0,
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
                    .child("Nobody has this role yet."),
            );
        }
        for (n, m) in holders.into_iter().enumerate() {
            let Some(user) = m.user.clone() else { continue };
            let (uid, rid) = (user.id.clone(), role.id.clone());
            let red = alpha(p.destructive, 0.1);
            out = out.child(motion::rise(
                row(p)
                    .p(px(8.0))
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
                                    .when_some(color, |el, c| el.text_color(color_of(c)))
                                    .child(member_name(m)),
                            )
                            .child(div().text_xs().text_color(p.muted_foreground).child(format!("@{}", user.username))),
                    )
                    .when(!locked, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("role-take-{}", user.id)))
                                .size(px(34.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(corner(10.0))
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
                8.0,
            ));
        }
        out
    }
}
