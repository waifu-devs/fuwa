//! The agents in a server, at the top of the Integrations page, for people
//! who manage it: who's here, adding one by username (or one of your own with
//! a tap), and taking one out. Where the instance offers MCP, also which of
//! them may use the server through it. The web's `settings/server/ServerAgents.tsx`.

use gpui_kit::AnimationExt as _;

use super::*;

pub(super) struct Agents {
    /// The agents you made, to add with a tap.
    mine: Option<Vec<pb::Agent>>,
    loading: bool,
    username: Entity<InputState>,
    /// "add" while one's being added, or the id of one being taken out.
    busy: Option<String>,
    confirming: Option<String>,
    /// The one just added, so it lights up for a moment.
    added: Option<(String, Instant)>,
    /// When adding didn't work, so the box shakes.
    shook: Option<Instant>,
    /// Which agents may use the server through MCP, once read.
    mcp: Option<pb::McpAccess>,
    mcp_loading: bool,
}

impl Agents {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("An agent's username"));
        let subscriptions =
            vec![cx.subscribe_in(&username, window, |this: &mut ServerSettingsView, s, e: &InputEvent, window, cx| {
                match e {
                    InputEvent::PressEnter { .. } => {
                        let name = s.read(cx).value().to_string();
                        this.add_agent(name, window, cx)
                    }
                    _ => cx.notify(),
                }
            })];
        let agents = Self {
            mine: None,
            loading: false,
            username,
            busy: None,
            confirming: None,
            added: None,
            shook: None,
            mcp: None,
            mcp_loading: false,
        };
        (agents, subscriptions)
    }
}

/// Who may use the server through MCP, as the web's choices name them.
const MCP_CHOICES: [(pb::McpAccessMode, &str); 3] = [
    (pb::McpAccessMode::All, "Every agent"),
    (pb::McpAccessMode::Chosen, "Only chosen"),
    (pb::McpAccessMode::Off, "None"),
];

/// What someone typed as a username: no @, no spaces, lowercase.
fn clean(name: &str) -> String {
    name.trim().trim_start_matches('@').to_lowercase()
}

impl ServerSettingsView {
    fn load_my_agents(&mut self, cx: &mut Context<Self>) {
        if self.agents.loading || self.agents.mine.is_some() {
            return;
        }
        self.agents.loading = true;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(cx, async move { core.my_agents(&key).await }, |this, result, cx| {
            this.agents.loading = false;
            this.agents.mine = Some(result.unwrap_or_default());
            cx.notify();
        });
    }

    fn load_mcp(&mut self, cx: &mut Context<Self>) {
        if self.agents.mcp_loading || self.agents.mcp.is_some() {
            return;
        }
        self.agents.mcp_loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.mcp_access(&key, &sid).await }, |this, result, cx| {
            // Left unread when it fails, so the choice just doesn't show.
            if let Ok(access) = result {
                this.agents.mcp = Some(access);
            }
            cx.notify();
        });
    }

    /// Shown at once, put back if the instance says no.
    fn save_mcp(&mut self, mode: pb::McpAccessMode, agent_ids: Vec<String>, cx: &mut Context<Self>) {
        let before = self.agents.mcp.clone();
        let access = pb::McpAccess { mode: mode as i32, agent_ids };
        self.agents.mcp = Some(access.clone());
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.set_mcp_access(&key, &sid, access).await }, move |this, result, cx| {
            match result {
                Ok(saved) => this.agents.mcp = Some(saved),
                Err(err) => {
                    this.agents.mcp = before.clone();
                    this.error = Some(err.message);
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    /// "Through MCP": every agent, only the chosen ones, or none.
    fn mcp_choice(
        &self,
        access: &pb::McpAccess,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = match access.mode() {
            pb::McpAccessMode::Unspecified => pb::McpAccessMode::All,
            mode => mode,
        };
        let width = 112.0;
        let chosen = MCP_CHOICES.iter().position(|(m, _)| *m == current).unwrap_or(0);
        let pill = gpui_kit::base::motion::spring(
            "mcp-choice",
            chosen as f32 * width,
            gpui_kit::base::motion::Spring::new(Duration::from_millis(340)).with_damping(0.75),
            window,
            cx,
        );
        let mut row = div()
            .relative()
            .flex()
            .p(px(4.0))
            .rounded(corner(12.0))
            .bg(alpha(p.muted_foreground, 0.1))
            .w(px(width * 3.0 + 8.0))
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .left(px(4.0 + pill))
                    .w(px(width))
                    .h(px(30.0))
                    .rounded(corner(9.0))
                    .bg(p.card),
            );
        for (mode, label) in MCP_CHOICES {
            let on = mode == current;
            let kept = access.agent_ids.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("mcp-{}", mode as i32)))
                    .relative()
                    .w(px(width))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if on { p.foreground } else { p.muted_foreground })
                    .when(!on, |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            // Chosen agents are kept for "Only chosen"; the others need none.
                            let ids = if mode == pb::McpAccessMode::Chosen { kept.clone() } else { Vec::new() };
                            this.save_mcp(mode, ids, cx);
                        }))
                    })
                    .child(label),
            );
        }
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.4))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(icon("plug-zap").size(px(16.0)).text_color(hsla(0.73, 0.7, 0.62, 1.0)))
                        .child(div().text_sm().font_weight(FontWeight::BOLD).child("Through MCP")),
                )
                .child(div().text_xs().text_color(p.muted_foreground).child(
                    "AI apps such as Claude can use this server through the instance's MCP endpoint with an agent's \
                     token. This only decides MCP: what an agent can do here is still up to its roles.",
                ))
                .child(row),
            "mcp-choice-in",
            Duration::ZERO,
            6.0,
        )
        .into_any_element()
    }

    fn add_agent(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.agents.busy.is_some() {
            return;
        }
        let name = clean(&name);
        if name.is_empty() {
            self.agents.shook = Some(Instant::now());
            cx.notify();
            return;
        }
        self.agents.busy = Some("add".into());
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let input = self.agents.username.clone();
        let rx = core.spawn({
            let core = core.clone();
            async move { core.add_agent(&key, &sid, &name).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.agents.busy = None;
                match result {
                    Ok(member) => {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        let id = member.user.map(|u| u.id).unwrap_or_default();
                        this.agents.added = Some((id, Instant::now()));
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(1900)).await;
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                    Err(err) => {
                        this.error = Some(err.message);
                        this.agents.shook = Some(Instant::now());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_agent(&mut self, id: String, cx: &mut Context<Self>) {
        self.agents.confirming = None;
        self.agents.busy = Some(id.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(
            cx,
            async move { core.moderate(&key, &sid, &id, Action::Kick, "Removed from Integrations").await },
            |this, result, cx| {
                this.agents.busy = None;
                if let Err(err) = result {
                    this.error = Some(err.message);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn agents_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.load_my_agents(cx);
        let mcp_on =
            self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.node.as_ref()).is_some_and(|n| n.mcp));
        if mcp_on {
            self.load_mcp(cx);
        }
        let here: Vec<pb::Member> = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.members.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|m| is_agent(m.user.as_ref()))
                .cloned()
                .collect()
        });
        let ids: Vec<&str> = here.iter().filter_map(|m| m.user.as_ref().map(|u| u.id.as_str())).collect();
        let yours: Vec<pb::User> = self
            .agents
            .mine
            .iter()
            .flatten()
            .filter_map(|a| a.user.clone())
            .filter(|u| !ids.contains(&u.id.as_str()))
            .collect();
        let adding = self.agents.busy.as_deref() == Some("add");

        let header = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .size(px(34.0))
                    .flex_none()
                    .rounded(corner(12.0))
                    .bg(hsla(0.73, 0.8, 0.6, 0.15))
                    .text_color(hsla(0.73, 0.7, if p.dark { 0.72 } else { 0.55 }, 1.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::once(
                        icon("bot").size(px(17.0)),
                        "agents-bot",
                        Duration::from_millis(1400),
                        |el, t| {
                            el.rotate(gpui_kit::radians((t * std::f32::consts::TAU * 1.5).sin() * 0.18 * (1.0 - t)))
                        },
                    )),
            )
            .child(
                div().flex_1().min_w_0().child(div().font_weight(FontWeight::EXTRA_BOLD).child("Agents")).child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child("Accounts programs drive. They talk like members, with the roles you give them."),
                ),
            );

        let field = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().flex_1().child(Input::new(&self.agents.username).prefix(icon("at-sign").size(px(15.0)))))
            .child(
                primary_button("agent-add", "Add", p)
                    .flex_none()
                    .when(adding, |el| el.opacity(0.6))
                    .child(if adding {
                        spinner("agent-add-spin", 15.0, window)
                    } else {
                        icon("plus").size(px(15.0)).into_any_element()
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        let name = this.agents.username.read(cx).value().to_string();
                        this.add_agent(name, window, cx)
                    })),
            );
        let field = match self.agents.shook.filter(|at| at.elapsed() < Duration::from_millis(450)) {
            Some(at) => field
                .with_animation(
                    SharedString::from(format!("agent-shake-{at:?}")),
                    gpui_kit::Animation::new(Duration::from_millis(350)),
                    |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.0).sin() * 8.0 * (1.0 - t))),
                )
                .into_any_element(),
            None => field.into_any_element(),
        };

        let mut section = div().flex().flex_col().gap(px(12.0)).child(header).child(field);

        if !yours.is_empty() {
            let mut row =
                div().flex().flex_wrap().items_center().gap(px(6.0)).child(
                    div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("Yours:"),
                );
            for (n, u) in yours.iter().enumerate() {
                let name = u.username.clone();
                let hover = alpha(p.primary, 0.06);
                let border = alpha(p.primary, 0.4);
                row = row.child(motion::rise(
                    div()
                        .id(SharedString::from(format!("agent-mine-{}", u.id)))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(28.0))
                        .pl(px(3.0))
                        .pr(px(10.0))
                        .rounded_full()
                        .border_1()
                        .border_color(p.border)
                        .bg(alpha(p.background, 0.6))
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .hover(move |s| s.bg(hover).border_color(border))
                        .active(|s| s.top(px(1.0)))
                        .child(avatar(Some(u), 20.0, p))
                        .child(user_name(u))
                        .child(icon("plus").size(px(12.0)).text_color(p.primary))
                        .on_click(cx.listener(move |this, _, window, cx| this.add_agent(name.clone(), window, cx))),
                    SharedString::from(format!("agent-mine-in-{}", u.id)),
                    Duration::from_millis(40 * n.min(8) as u64),
                    6.0,
                ));
            }
            section = section.child(row);
        }

        if let Some(access) = self.agents.mcp.clone().filter(|_| mcp_on) {
            section = section.child(self.mcp_choice(&access, p, window, cx));
        }

        if here.is_empty() {
            section = section.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(corner(16.0))
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(motion::once(
                        div().text_size(px(26.0)).child("🤖"),
                        "agents-empty-bob",
                        Duration::from_millis(1200),
                        |el, t| el.relative().top(px(-5.0 * (t * std::f32::consts::PI).sin())),
                    ))
                    .child(div().flex_1().min_w_0().child(
                        "No agents here yet. Add one by its username; agents are made under Settings, Agents in \
                         the web app.",
                    )),
                "agents-empty",
                Duration::ZERO,
                6.0,
            ));
        } else {
            let mut list = div().flex().flex_col().gap(px(8.0));
            for (n, m) in here.iter().enumerate() {
                list = list.child(self.agent_row(m, n, p, window, cx));
            }
            section = section.child(list);
        }
        section.into_any_element()
    }

    fn agent_row(
        &self,
        m: &pb::Member,
        n: usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let user = m.user.clone().unwrap_or_default();
        let id = user.id.clone();
        let fresh =
            self.agents.added.as_ref().is_some_and(|(a, at)| *a == id && at.elapsed() < Duration::from_millis(1800));
        let confirming = self.agents.confirming.as_deref() == Some(id.as_str());
        let removing = self.agents.busy.as_deref() == Some(id.as_str());
        let shown = if m.nickname.is_empty() { user_name(&user) } else { m.nickname.clone() };
        let added = m.joined_at.as_ref().map(|t| format!(" · added {}", stamp(t.seconds * 1000))).unwrap_or_default();

        let face = avatar(Some(&user), 36.0, p);
        let face = if fresh {
            motion::once(face, SharedString::from(format!("agent-hi-{id}")), Duration::from_millis(600), |el, t| {
                let k = (t * std::f32::consts::PI).sin();
                el.relative().top(px(-5.0 * k)).left(px((t * std::f32::consts::TAU * 1.5).sin() * 3.0 * (1.0 - t)))
            })
        } else {
            face.into_any_element()
        };

        let end: AnyElement = if confirming {
            let (yes, no) = (id.clone(), id.clone());
            motion::slide_in(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        danger_button(SharedString::from(format!("agent-out-yes-{yes}")), "Remove", p)
                            .h(px(32.0))
                            .px(px(12.0))
                            .text_xs()
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_agent(yes.clone(), cx))),
                    )
                    .child(icon_button(SharedString::from(format!("agent-out-no-{no}")), "x", p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.agents.confirming = None;
                            cx.notify();
                        }),
                    )),
                SharedString::from(format!("agent-ask-{id}")),
                8.0,
            )
            .into_any_element()
        } else {
            let ask = id.clone();
            let red = alpha(p.destructive, 0.1);
            let c = p.destructive;
            div()
                .id(SharedString::from(format!("agent-out-{id}")))
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(34.0))
                .px(px(10.0))
                .rounded(corner(12.0))
                .cursor_pointer()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .hover(move |s| s.bg(red).text_color(c))
                .child(if removing {
                    spinner(format!("agent-out-spin-{id}"), 14.0, window)
                } else {
                    icon("user-minus").size(px(14.0)).into_any_element()
                })
                .child("Remove")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.agents.busy.is_none() {
                        this.agents.confirming = Some(ask.clone());
                        cx.notify();
                    }
                }))
                .into_any_element()
        };

        // With "Only chosen", each agent has its own switch.
        let mcp_on =
            self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.node.as_ref()).is_some_and(|n| n.mcp));
        let mcp_switch =
            self.agents.mcp.as_ref().filter(|a| mcp_on && a.mode() == pb::McpAccessMode::Chosen).map(|access| {
                let ids = access.agent_ids.clone();
                let on = ids.contains(&id);
                let who = id.clone();
                motion::rise(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child("MCP")
                        .child(switch(
                            SharedString::from(format!("agent-mcp-{id}")),
                            on,
                            false,
                            cx,
                            move |this: &mut Self, on, cx| {
                                let mut ids = ids.clone();
                                ids.retain(|i| *i != who);
                                if on {
                                    ids.push(who.clone());
                                }
                                this.save_mcp(pb::McpAccessMode::Chosen, ids, cx);
                            },
                        )),
                    SharedString::from(format!("agent-mcp-in-{id}")),
                    Duration::ZERO,
                    4.0,
                )
            });

        let hover = alpha(p.primary, 0.3);
        motion::rise(
            div()
                .id(SharedString::from(format!("agent-{id}")))
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(10.0))
                .rounded(corner(16.0))
                .border_1()
                .map(|el| {
                    if fresh {
                        el.border_color(alpha(p.success, 0.5)).bg(alpha(p.success, 0.06))
                    } else {
                        el.border_color(p.border).bg(alpha(p.background, 0.5)).hover(move |s| s.border_color(hover))
                    }
                })
                .child(face)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(div().min_w_0().truncate().font_weight(FontWeight::BOLD).child(shown))
                                .child(app_badge(SharedString::from(format!("agent-badge-{id}")), "AGENT", p))
                                .when(fresh, |el| {
                                    el.child(motion::rise(
                                        icon("check").size(px(14.0)).text_color(p.success),
                                        SharedString::from(format!("agent-new-{id}")),
                                        Duration::ZERO,
                                        6.0,
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child(format!("@{}{added}", user.username)),
                        ),
                )
                .children(mcp_switch)
                .child(end),
            SharedString::from(format!("agent-in-{id}")),
            Duration::from_millis(30 * n.min(8) as u64),
            8.0,
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn usernames_are_cleaned() {
        assert_eq!(super::clean("  @Robo.Cat "), "robo.cat");
        assert_eq!(super::clean("@"), "");
    }
}
