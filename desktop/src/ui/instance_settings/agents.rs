//! The Agents page: the instance's public agents, as Browse lists them,
//! each with the switch that features it in Browse and, once featured, its
//! place among the featured with arrows to move it, like the feature switch
//! on the Servers page. Over `AgentService.DiscoverAgents` and
//! `AdminService.SetFeaturedAgents`; only on instances with `agent-discovery`.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};

use super::InstanceSettingsView;
use super::controls::{shimmer, switch, tracked_count};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_servers as servers;
use crate::core::store::user_name;
use crate::pb;
use crate::ui::motion;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::{avatar, icon};

#[derive(Default)]
pub(super) struct Agents {
    /// The public agents, featured first, once asked for.
    list: Option<Vec<pb::AgentListing>>,
    /// The agents featured in Browse, in their order, and whether a change to it is on its way.
    featured: Vec<String>,
    featuring: bool,
    error: Option<String>,
    asked: bool,
}

fn id_of(a: &pb::AgentListing) -> String {
    a.user.as_ref().map(|u| u.id.clone()).unwrap_or_default()
}

fn capitalized(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

impl InstanceSettingsView {
    fn load_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agents.asked = true;
        self.agents.error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.discover_agents(&key).await }, |this, result, _, cx| {
            match result {
                Ok(found) => {
                    this.agents.list = Some(found.agents);
                    this.agents.featured = found.featured;
                }
                Err(problem) => this.agents.error = Some(problem.message),
            }
            cx.notify();
        });
    }

    /// Sends the whole featured list as `ids`, and keeps what the instance answers.
    fn set_featured_agents(
        &mut self,
        ids: Vec<String>,
        toast: Option<(bool, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agents.featuring {
            return;
        }
        self.agents.featuring = true;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.set_featured_agents(&key, ids).await }, move |this, result, _, cx| {
            this.agents.featuring = false;
            match result {
                Ok(now) => {
                    this.agents.featured = now;
                    if let Some((on, name)) = toast {
                        let k = if on { "desktop.agents.featuredNow" } else { "desktop.agents.unfeaturedNow" };
                        this.toast("star", t_with(k, &[("agent", Arg::Str(&name))]), cx);
                    }
                }
                Err(problem) => this.toast("circle-alert", problem.message, cx),
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn agents_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.agents.asked {
            self.load_agents(window, cx);
        }
        if let Some(error) = &self.agents.error {
            return div().text_sm().text_color(p.muted_foreground).child(capitalized(error)).into_any_element();
        }
        let Some(list) = self.agents.list.clone() else {
            return div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children((0..3).map(|n| shimmer(n, 64.0, p)))
                .into_any_element();
        };
        // Featured first, in their order, then the rest as Browse lists them.
        let featured = self.agents.featured.clone();
        let mut shown: Vec<&pb::AgentListing> =
            featured.iter().filter_map(|id| list.iter().find(|a| id_of(a) == *id)).collect();
        shown.extend(list.iter().filter(|a| !featured.contains(&id_of(a))));
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(icon("bot").size(px(14.0)))
            .child(tracked_count("instance-agents-count", "desktop.agents.adminCount", shown.len() as u64, 12.0));
        let mut rows = div().flex().flex_col().gap(px(6.0));
        for (n, agent) in shown.iter().enumerate() {
            rows = rows.child(self.agent_row(agent, n, p, window, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(t("desktop.agents.adminAbout")),
            )
            .child(header)
            .child(rows)
            .when(shown.is_empty(), |el| {
                el.child(motion::rise(
                    div()
                        .py(px(32.0))
                        .flex()
                        .justify_center()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("desktop.agents.adminNone")),
                    "agents-none",
                    Duration::ZERO,
                    8.0,
                ))
            })
            .into_any_element()
    }

    /// One public agent: who it is, the switch, and its place when featured.
    fn agent_row(
        &mut self,
        agent: &pb::AgentListing,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let user = agent.user.clone().unwrap_or_default();
        let id = user.id.clone();
        let name = user_name(&user);
        let featured = &self.agents.featured;
        let place = featured.iter().position(|f| *f == id);
        let count = featured.len();
        let busy = self.agents.featuring;
        let (aid, toast_name) = (id.clone(), name.clone());
        let toggle =
            switch(format!("feature-agent-{id}"), place.is_some(), busy, p, window, cx, move |this, on, window, cx| {
                let ids = servers::featuring(&this.agents.featured, &aid, on);
                this.set_featured_agents(ids, Some((on, toast_name.clone())), window, cx);
            });
        let owner = agent.owner.as_ref().map(user_name).unwrap_or_default();
        let facts = format!(
            "@{} · {}{}",
            user.username,
            t_with("accountsettings.agents.servers", &[("count", Arg::Num(i64::from(agent.servers)))]),
            if owner.is_empty() {
                String::new()
            } else {
                format!(" · {}", t_with("desktop.agents.madeBy", &[("name", Arg::Str(&owner))]))
            }
        );
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(avatar(Some(&user), 40.0, p))
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
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .when(place.is_some(), |el| el.child(icon("star").size(px(14.0)).text_color(p.primary)))
                            .child(div().min_w_0().truncate().child(name)),
                    )
                    .child(
                        div().truncate().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(facts),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(t("instancesettings.servers.feature"))
                    .child(toggle),
            );
        let mut row = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(if place.is_some() { alpha(p.primary, 0.4) } else { p.border.into() })
            .bg(alpha(p.background, 0.5))
            .child(head);
        if let Some(at) = place.filter(|_| count >= 2) {
            let where_now = motion::swap_text(
                SharedString::from(format!("feature-agent-place-{id}")),
                t_with(
                    "instancesettings.servers.featurePlace",
                    &[("place", Arg::Num(at as i64 + 1)), ("count", Arg::Num(count as i64))],
                ),
                12.0,
                window,
                cx,
            );
            let step = |earlier: bool| {
                let (key, glyph, end) = if earlier {
                    ("instancesettings.servers.featureEarlier", "arrow-up", at == 0)
                } else {
                    ("instancesettings.servers.featureLater", "arrow-down", at + 1 == count)
                };
                let off = end || busy;
                let aid = id.clone();
                button(
                    format!("feature-agent-{}-{id}", if earlier { "up" } else { "down" }),
                    t(key),
                    Some(glyph),
                    Look::Outline,
                    true,
                    p,
                )
                .rounded(radius_xl())
                .when(off, |el| el.opacity(0.5))
                .when(!off, |el| {
                    el.on_click(cx.listener(move |this, _, window, cx| {
                        let ids = servers::featured_moved(&this.agents.featured, &aid, earlier);
                        this.set_featured_agents(ids, None, window, cx);
                    }))
                })
            };
            row = row.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(where_now))
                    .child(div().flex().gap(px(6.0)).child(step(true)).child(step(false))),
                SharedString::from(format!("feature-agent-order-{id}")),
                Duration::ZERO,
                6.0,
            ));
        }
        motion::rise(
            row,
            SharedString::from(format!("agent-row-{id}")),
            Duration::from_millis(30 * n.min(10) as u64),
            8.0,
        )
        .into_any_element()
    }
}
