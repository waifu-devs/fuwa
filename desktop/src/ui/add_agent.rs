//! Agents anyone here can add (only where the instance has
//! `agent-discovery`): the Agents section in Browse (the public ones, those
//! the admins feature first and big), and adding one to a server you manage,
//! a dialog with the agent's card (picture, name, what it's about, who made
//! it, how many servers it's in) over those servers, each with its Add
//! button. It opens from Browse, from an agent's profile card, and from its
//! invite link (`<instance>/agent/<username>`) pasted in Browse's invite box.
//! Over `AgentService.DiscoverAgents`, `LookUpAgent` and `AddAgent`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, BoxShadow, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
};

use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::join::{self, DiscoveredAgents};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Nav};
use crate::ui::instance_home::{capitalized, filled_button, inline_markdown, outline_button, shake, shimmer, spinner};
use crate::ui::motion;
use crate::ui::text::{TIGHT, WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::{app_badge, avatar, icon, is_agent, server_icon};

/// Rows of the agents that aren't featured shown before "Show all".
const FIRST_ROWS: usize = 2;

/// What Browse's agents and the add dialog keep between draws.
#[derive(Default)]
pub struct Agents {
    /// The agent being added (`Dialog::AddAgent`).
    pub page: Option<AgentPage>,
    /// Browse's agents by instance; missing while they're asked for.
    browse: HashMap<String, Result<DiscoveredAgents, String>>,
    /// Instances whose Browse shows every agent, not just the first rows.
    all: HashSet<String>,
    /// Instances whose Browse is on its Agents tab rather than Servers.
    pub(crate) tab: HashSet<String>,
    /// What profile cards learned by "key|agent": the agent when you may add
    /// it, None while it's asked or when you may not.
    listings: HashMap<String, Option<pb::AgentListing>>,
}

/// The agent the add dialog is about.
pub struct AgentPage {
    pub key: String,
    username: String,
    found: Option<Result<pb::AgentListing, Problem>>,
    /// Adds on their way, done, and what went wrong, by server.
    adding: HashSet<String>,
    added: HashSet<String>,
    errors: HashMap<String, String>,
    /// The roles picked for it to join each server with.
    roles: HashMap<String, HashSet<String>>,
}

fn listing_user(a: &pb::AgentListing) -> pb::User {
    a.user.clone().unwrap_or_default()
}

impl FuwaApp {
    // ───────────────────────── Acting ─────────────────────────

    /// Asks for the instance's public agents, for Browse.
    pub(crate) fn load_browse_agents(&mut self, key: &str, cx: &mut Context<Self>) {
        let has = self.core.shared.read(|s| s.instance(key).is_some_and(|i| i.has("agent-discovery")));
        if !has {
            return;
        }
        let (core, k) = (self.core.clone(), key.to_owned());
        self.run(cx, async move { core.discover_agents(&k).await }, {
            let key = key.to_owned();
            move |this, result, cx| {
                this.home.agents.browse.insert(key, result.map_err(|e| e.message));
                cx.notify();
            }
        });
    }

    /// Opens the dialog for adding an agent, found by its username, or
    /// already known (Browse, a profile card).
    pub(crate) fn open_add_agent(
        &mut self,
        key: &str,
        username: &str,
        known: Option<pb::AgentListing>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let asks = known.is_none();
        self.home.agents.page = Some(AgentPage {
            key: key.to_owned(),
            username: username.to_owned(),
            found: known.map(Ok),
            adding: HashSet::new(),
            added: HashSet::new(),
            errors: HashMap::new(),
            roles: HashMap::new(),
        });
        self.open_dialog(Dialog::AddAgent { key: key.to_owned() }, window, cx);
        if asks {
            let (core, k, name) = (self.core.clone(), key.to_owned(), username.to_owned());
            let asked = name.clone();
            self.run(cx, async move { core.look_up_agent(&k, "", &asked).await }, move |this, result, cx| {
                if let Some(page) = this.home.agents.page.as_mut().filter(|p| p.username == name) {
                    page.found = Some(result);
                }
                cx.notify();
            });
        }
    }

    /// An agent's link pasted in Browse: its dialog, on an instance you're signed in to.
    pub(crate) fn open_agent_link(&mut self, at: &str, username: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.core.shared.read(|s| s.instance(at).is_some_and(|i| i.me.is_some()));
        if !ready {
            let host = if self.prefs.streamer_mode { "•••••".to_owned() } else { join::address_of(at) };
            let title = t_with("desktop.agents.signInFirst", &[("host", Arg::Str(&host))]);
            self.toast("bot", title, String::new(), None, None, cx);
            return;
        }
        self.open_add_agent(at, username, None, window, cx);
    }

    /// Adds the open dialog's agent to a server; a tick on its row once it's in.
    fn add_agent_to(&mut self, server: pb::Server, cx: &mut Context<Self>) {
        let Some(page) = self.home.agents.page.as_mut() else { return };
        let Some(Ok(agent)) = page.found.clone() else { return };
        if !page.adding.insert(server.id.clone()) {
            return;
        }
        page.errors.remove(&server.id);
        let key = page.key.clone();
        let roles: Vec<String> = page.roles.get(&server.id).into_iter().flatten().cloned().collect();
        let user = listing_user(&agent);
        let (core, k, sid, username) = (self.core.clone(), key.clone(), server.id.clone(), user.username.clone());
        let adding = async move {
            if roles.is_empty() {
                core.add_agent(&k, &sid, &username).await
            } else {
                core.add_agent_with_roles(&k, &sid, &username, roles).await
            }
        };
        self.run(cx, adding, move |this, result, cx| {
            let Some(page) = this.home.agents.page.as_mut().filter(|p| p.key == key) else { return };
            page.adding.remove(&server.id);
            match result {
                Ok(_) => {
                    page.added.insert(server.id.clone());
                    if let Some(Ok(a)) = page.found.as_mut() {
                        a.servers += 1;
                    }
                    let title = t_with(
                        "desktop.agents.addedTo",
                        &[("agent", Arg::Str(&user_name(&user))), ("server", Arg::Str(&server.name))],
                    );
                    let open = Some(Nav::Server { key: key.clone(), server: server.id.clone() });
                    this.toast("bot", title, String::new(), open, None, cx);
                }
                // Already a member there (its members weren't loaded here): as good as added.
                Err(e) if e.code == tonic::Code::AlreadyExists => {
                    page.added.insert(server.id.clone());
                }
                Err(e) => {
                    page.errors.insert(server.id.clone(), e.message);
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    /// For an agent's profile card: the agent, when you manage a server here
    /// and may add it there (it's public, or yours). Asks once, the first time.
    pub(crate) fn addable_agent(
        &mut self,
        key: &str,
        user: Option<&pb::User>,
        cx: &mut Context<Self>,
    ) -> Option<pb::AgentListing> {
        if !is_agent(user) {
            return None;
        }
        let user = user?;
        let manages = self.core.shared.read(|s| {
            s.instance(key).is_some_and(|i| {
                i.has("agent-discovery")
                    && i.me.as_ref().is_some_and(|m| m.id != user.id)
                    && !join::managed_servers(i, &user.id).is_empty()
            })
        });
        if !manages {
            return None;
        }
        let tag = format!("{key}|{}", user.id);
        if let Some(known) = self.home.agents.listings.get(&tag) {
            return known.clone();
        }
        self.home.agents.listings.insert(tag.clone(), None);
        let (core, k, id) = (self.core.clone(), key.to_owned(), user.id.clone());
        self.run(cx, async move { core.look_up_agent(&k, &id, "").await }, move |this, result, cx| {
            // Not found (someone else's private agent) or unreachable: no button.
            this.home.agents.listings.insert(tag, result.ok());
            cx.notify();
        });
        None
    }

    // ───────────────────────── The dialog ─────────────────────────

    /// The add dialog's body: the agent, then the servers you manage.
    pub(crate) fn add_agent_body(
        &mut self,
        key: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(page) = self.home.agents.page.as_ref().filter(|pg| pg.key == key) else {
            return div().into_any_element();
        };
        let agent = match page.found.clone() {
            None => {
                return div()
                    .h(px(240.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .child(spinner(24.0, "add-agent-loading", window))
                    .into_any_element();
            }
            Some(Err(problem)) => return self.agent_missing(&problem, p, cx),
            Some(Ok(agent)) => agent,
        };
        let user = listing_user(&agent);
        let (adding, added, errors) = (page.adding.clone(), page.added.clone(), page.errors.clone());
        let picked = page.roles.clone();
        // Each server you manage, whether it's in already, and the roles it could join with.
        let managed: Vec<(pb::Server, bool, Vec<pb::Role>)> = self
            .core
            .shared
            .read(|s| {
                s.instance(key).map(|i| {
                    join::managed_servers(i, &user.id)
                        .into_iter()
                        .map(|(server, here)| {
                            let roles = join::assignable_roles(i, &server.id);
                            (server, here, roles)
                        })
                        .collect()
                })
            })
            .unwrap_or_default();

        let mut body = div().flex().flex_col().gap(px(16.0)).p(px(24.0)).child(agent_head(&agent, true, p));
        if !agent.public {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .bg(p.muted)
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(icon("lock").size(px(12.0)))
                    .child(t("desktop.agents.private")),
            );
        }
        let mut list = div().flex().flex_col().gap(px(6.0));
        if managed.is_empty() {
            list = list.child(
                div()
                    .py(px(12.0))
                    .text_center()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("accountsettings.agents.noManaged")),
            );
        }
        for (n, (server, here, roles)) in managed.into_iter().enumerate() {
            let busy = adding.contains(&server.id);
            let done = added.contains(&server.id);
            let error = errors.get(&server.id).cloned();
            let sid = server.id.clone();
            let action: AnyElement = if here && !done {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(icon("check").size(px(14.0)))
                    .child(t("desktop.agents.alreadyIn"))
                    .into_any_element()
            } else if done {
                let green = gpui_kit::rgb(0x10b981);
                motion::pop(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(green)
                        .child(icon("check").size(px(14.0)))
                        .child(t("accountsettings.agents.added")),
                    SharedString::from(format!("add-agent-done|{sid}")),
                    0.6,
                    0.0,
                    Duration::ZERO,
                )
                .into_any_element()
            } else {
                let s = server.clone();
                filled_button(SharedString::from(format!("add-agent|{sid}")), 32.0, p.primary, p.primary_foreground, p)
                    .w_auto()
                    .px(px(14.0))
                    .text_size(px(13.0))
                    .when(busy, |el| el.opacity(0.5).child(spinner(14.0, &format!("add-agent-busy|{sid}"), window)))
                    .when(!busy, |el| el.child(icon("plus").size(px(14.0))))
                    .child(t("desktop.agents.add"))
                    .when(!busy, |el| el.on_click(cx.listener(move |this, _, _, cx| this.add_agent_to(s.clone(), cx))))
                    .into_any_element()
            };
            let row = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(if done { alpha(gpui_kit::rgb(0x10b981), 0.4) } else { p.border.into() })
                .bg(alpha(p.background, 0.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(server_icon(&server, 36.0, 12.0, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(server.name.clone()),
                        )
                        .child(action),
                )
                .when(!here && !done && !roles.is_empty(), |el| {
                    el.child(self.role_chips(&server.id, &roles, picked.get(&server.id), busy, p, cx))
                })
                .when_some(error, |el, error| {
                    el.child(motion::once(
                        div().text_xs().line_height(px(16.0)).text_color(p.destructive).child(capitalized(&error)),
                        SharedString::from(format!("add-agent-error|{sid}|{error}")),
                        Duration::from_millis(800),
                        |el, t| el.relative().left(px(shake(t) * 0.75)),
                    ))
                });
            list = list.child(motion::rise(
                row,
                SharedString::from(format!("add-agent-row|{sid}")),
                Duration::from_millis(40 * n.min(8) as u64),
                8.0,
            ));
        }
        body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .pt(px(16.0))
                .border_t_1()
                .border_color(p.border)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(tracked(t("accountsettings.agents.managed").to_uppercase(), WIDE)),
                )
                .child(list),
        )
        .into_any_element()
    }

    /// The roles an agent may join a server with, picked by a tap each.
    fn role_chips(
        &mut self,
        server_id: &str,
        roles: &[pb::Role],
        picked: Option<&HashSet<String>>,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chips = div().flex().flex_wrap().items_center().gap(px(6.0)).child(
            div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(t("desktop.agents.withRoles")),
        );
        for role in roles {
            let on = picked.is_some_and(|set| set.contains(&role.id));
            let (sid, rid) = (server_id.to_owned(), role.id.clone());
            let tint = alpha(p.primary, 0.15);
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("add-agent-role|{server_id}|{}", role.id)))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(24.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if on { alpha(p.primary, 0.6) } else { p.border.into() })
                    .when(on, |el| el.bg(tint))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(crate::ui::profile_card::role_dot(role, p))
                    .child(role.name.clone())
                    .when(on, |el| el.child(icon("check").size(px(12.0))))
                    .when(!busy, |el| {
                        el.cursor_pointer().active(|s| s.scale(0.95)).on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(page) = this.home.agents.page.as_mut() {
                                let set = page.roles.entry(sid.clone()).or_default();
                                if !set.remove(&rid) {
                                    set.insert(rid.clone());
                                }
                            }
                            cx.notify();
                        }))
                    }),
            );
        }
        chips.into_any_element()
    }

    /// An agent link that leads nowhere: no such agent, or someone else's private one.
    fn agent_missing(&mut self, problem: &Problem, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let gone = problem.code == tonic::Code::NotFound;
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .p(px(32.0))
            .text_center()
            .child(
                div()
                    .size(px(64.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.muted)
                    .text_color(p.muted_foreground)
                    .child(icon("bot-off").size(px(28.0))),
            )
            .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(t("desktop.agents.missing")))
            .child(div().text_sm().text_color(p.muted_foreground).child(if gone {
                t("desktop.agents.missingAbout")
            } else {
                capitalized(&problem.message)
            }))
            .child(
                filled_button("add-agent-close", 36.0, p.primary, p.primary_foreground, p)
                    .w_auto()
                    .mt(px(4.0))
                    .child(t("common.close"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
            )
            .into_any_element()
    }

    // ───────────────────────── Browse ─────────────────────────

    /// Browse's Agents: the featured ones big, two to a row, then the rest
    /// as small cards, the first rows of them until "Show all". Nothing on
    /// instances without agent discovery or without public agents.
    pub(crate) fn browse_agents(
        &mut self,
        key: &str,
        width: f32,
        cols: u16,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let found = match self.home.agents.browse.get(key) {
            None => {
                return div()
                    .flex()
                    .gap(px(16.0))
                    .children(
                        (0..usize::from(cols))
                            .map(|n| div().flex_1().child(shimmer(176.0, radius_3xl(), p, window, n))),
                    )
                    .into_any_element();
            }
            Some(Err(error)) => {
                return div().text_sm().text_color(p.muted_foreground).child(capitalized(error)).into_any_element();
            }
            Some(Ok(found)) => found.clone(),
        };
        let query = self.home.search.read(cx).value().to_string();
        let (featured, rest) = join::agents_split(&found, &query);
        let (featured, rest): (Vec<pb::AgentListing>, Vec<pb::AgentListing>) =
            (featured.into_iter().cloned().collect(), rest.into_iter().cloned().collect());
        if featured.is_empty() && rest.is_empty() {
            let none = found.agents.is_empty();
            return div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .p(px(40.0))
                .rounded(radius_3xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .text_center()
                .when(none, |el| el.child(icon("bot").size(px(24.0)).mb(px(8.0)).text_color(p.primary)))
                .child(div().font_weight(FontWeight::BOLD).child(if none {
                    t("workspace.agents.noneTitle")
                } else {
                    t("workspace.home.noMatch")
                }))
                .child(div().mt(px(4.0)).text_sm().text_color(p.muted_foreground).child(if none {
                    t("workspace.agents.none")
                } else {
                    t("workspace.home.tryAnother")
                }))
                .into_any_element();
        }
        let about = div().mb(px(16.0)).text_sm().text_color(p.muted_foreground).child(t("desktop.agents.about"));
        let mut section = div().flex().flex_col().child(about);
        // The featured ones: two to a row where there's room.
        if !featured.is_empty() {
            let per_row = if cols >= 2 { 2 } else { 1 };
            let w = (width - 16.0 * (per_row as f32 - 1.0)) / per_row as f32;
            let mut rows = div().mb(px(16.0)).flex().flex_col().gap(px(16.0));
            for (r, chunk) in featured.chunks(per_row).enumerate() {
                let mut row = div().flex().gap(px(16.0));
                for (c, a) in chunk.iter().enumerate() {
                    let n = r * per_row + c;
                    let id = listing_user(a).id;
                    row = row.child(motion::rise(
                        div().w(px(w)).flex_none().flex().flex_col().child(self.agent_card(key, a, true, p, cx)),
                        SharedString::from(format!("agent-featured|{key}|{id}")),
                        Duration::from_millis(70 * n.min(6) as u64),
                        24.0,
                    ));
                }
                rows = rows.child(row);
            }
            section = section.child(rows);
        }
        if !rest.is_empty() {
            let cols = usize::from(cols.max(1));
            let all = self.home.agents.all.contains(key);
            let shown = if all { rest.len() } else { rest.len().min(cols * FIRST_ROWS) };
            let w = (width - 16.0 * (cols as f32 - 1.0)) / cols as f32;
            let mut rows = div().flex().flex_col().gap(px(16.0));
            for (r, chunk) in rest[..shown].chunks(cols).enumerate() {
                let mut row = div().flex().gap(px(16.0));
                for (c, a) in chunk.iter().enumerate() {
                    let n = r * cols + c;
                    let id = listing_user(a).id;
                    row = row.child(motion::rise(
                        div().w(px(w)).flex_none().flex().flex_col().child(self.agent_card(key, a, false, p, cx)),
                        SharedString::from(format!("agent-card|{key}|{id}")),
                        Duration::from_millis(50 * n.min(8) as u64),
                        20.0,
                    ));
                }
                rows = rows.child(row);
            }
            section = section.child(rows);
            if shown < rest.len() {
                let k = key.to_owned();
                section = section.child(
                    div().mt(px(12.0)).flex().justify_center().child(
                        outline_button("agents-all", 36.0, p)
                            .w_auto()
                            .child(t_with("desktop.agents.showAll", &[("count", Arg::Num(rest.len() as i64))]))
                            .child(icon("chevron-down").size(px(16.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.home.agents.all.insert(k.clone());
                                cx.notify();
                            })),
                    ),
                );
            }
        }
        motion::rise(section, SharedString::from(format!("agents|{key}")), Duration::ZERO, 12.0).into_any_element()
    }

    /// An agent in Browse: a featured one bigger, with more of what it's
    /// about. Add opens the dialog for picking a server.
    fn agent_card(
        &mut self,
        key: &str,
        agent: &pb::AgentListing,
        big: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let user = listing_user(agent);
        let border = if big { alpha(p.primary, 0.3) } else { p.border.into() };
        let hover = p.primary;
        let glow = alpha(p.primary, 0.55);
        let (k, a, name) = (key.to_owned(), agent.clone(), user.username.clone());
        let add = if big {
            filled_button(
                SharedString::from(format!("agent-add|{}", user.id)),
                40.0,
                p.primary,
                p.primary_foreground,
                p,
            )
        } else {
            outline_button(SharedString::from(format!("agent-add|{}", user.id)), 36.0, p)
        }
        .child(icon("plus").size(px(16.0)))
        .child(t("accountsettings.agents.addToServer"))
        .on_click(cx.listener(move |this, _, window, cx| this.open_add_agent(&k, &name, Some(a.clone()), window, cx)));
        div()
            .id(SharedString::from(format!("agent-browse|{key}|{}|{big}", user.id)))
            .relative()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(if big { 24.0 } else { 20.0 }))
            .rounded(radius_3xl())
            .border_1()
            .border_color(border)
            .bg(p.card)
            .when(big, |el| {
                el.shadow(vec![BoxShadow {
                    color: alpha(p.primary, 0.08),
                    offset: point(px(0.0), px(10.0)),
                    blur_radius: px(15.0),
                    spread_radius: px(-3.0),
                    inset: false,
                }])
            })
            .hover(move |s| {
                s.border_color(hover).translate_y(px(-4.0)).shadow(vec![BoxShadow {
                    color: glow,
                    offset: point(px(0.0), px(18.0)),
                    blur_radius: px(20.0),
                    spread_radius: px(-22.0),
                    inset: false,
                }])
            })
            .when(big, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(16.0))
                        .right(px(16.0))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(alpha(p.primary, 0.12))
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(p.primary)
                        .child(icon("star").size(px(14.0)))
                        .child(t("workspace.home.featured")),
                )
            })
            .child(agent_head(agent, big, p))
            .child(div().flex_1())
            .child(add)
            .into_any_element()
    }
}

/// An agent's picture, name and badge, its username, who made it and how
/// many servers it's in, and what it's about. `big` for the dialog and
/// featured cards.
fn agent_head(agent: &pb::AgentListing, big: bool, p: &Palette) -> gpui_kit::Div {
    let user = listing_user(agent);
    let face = if big { 72.0 } else { 48.0 };
    let owner = agent.owner.clone();
    let made = owner.as_ref().map(|o| {
        let name = user_name(o);
        let template = crate::ui::instance_home::template_text("desktop.agents.madeBy");
        div().flex().items_center().gap(px(4.0)).child(avatar(Some(o), 16.0, p)).child(crate::ui::text::hint_line(
            &template,
            &[("name", &name)],
            p,
        ))
    });
    let facts = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(px(12.0))
        .gap_y(px(4.0))
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(p.muted_foreground)
        .children(made)
        .child(div().flex().items_center().gap(px(4.0)).child(icon("server").size(px(14.0))).child(motion::counted(
            format!("agent-servers|{}|{big}", user.id),
            "accountsettings.agents.servers",
            agent.servers.max(0) as u64,
            12.0,
        )));
    let bio = agent.bio.trim();
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(if big { 16.0 } else { 12.0 }))
                .child(avatar(Some(&user), face, p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(if big { 22.0 } else { 16.0 }))
                                        .line_height(px(if big { 28.0 } else { 24.0 }))
                                        .font_weight(FontWeight::EXTRA_BOLD)
                                        .child(tracked(user_name(&user), TIGHT)),
                                )
                                .child(app_badge(
                                    SharedString::from(format!("agent-badge|{}|{big}", user.id)),
                                    "AGENT",
                                    p,
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(p.muted_foreground)
                                .child(format!("@{}", user.username)),
                        ),
                ),
        )
        .child(facts)
        .when(!bio.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .line_clamp(if big { 4 } else { 2 })
                    .child(inline_markdown(bio, p.foreground)),
            )
        })
}
