//! Your agents on the instance on screen, as the web's
//! `settings/account/Agents.tsx`: accounts your programs drive. Make one
//! (its token shows once), rename it, write its about, make it public, add
//! it to a server you manage, give it a new token, or delete it.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{Look, button, field, toggle};
use crate::ui::settings_menu::Item;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{avatar, icon};

const VIOLET: u32 = 0x8b5cf6;
const AMBER: u32 = 0xf59e0b;

#[derive(Default)]
pub(crate) struct AgentsForm {
    loaded: Option<String>,
    list: Option<Vec<pb::Agent>>,
    open: Option<String>,
    making: bool,
    how_to: bool,
    /// A token just made, shown until it's put away.
    fresh: Option<(String, String)>,
    token_shown: bool,
    copied: bool,
    name: Option<Entity<InputState>>,
    username: Option<Entity<InputState>>,
    edited_username: bool,
    busy: Option<&'static str>,
    confirm: Option<&'static str>,
    added: Option<String>,
    shake: Option<std::time::Instant>,
    /// The open agent's fields, and whose they are.
    edit_for: Option<String>,
    edit_name: Option<Entity<InputState>>,
    edit_bio: Option<Entity<TextareaState>>,
}

fn valid_username(u: &str) -> bool {
    let b = u.as_bytes();
    (2..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'.')
}

fn suggest(name: &str) -> String {
    let s: String = name.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join("_");
    let s: String =
        s.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '.').collect();
    s.trim_start_matches(['_', '.']).chars().take(32).collect()
}

fn ago(ms: i64) -> String {
    let minutes = ((crate::core::dms::now_ms() - ms) / 60_000).max(0);
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes} minutes ago")
    } else if minutes < 60 * 24 {
        format!("{} hours ago", minutes / 60)
    } else {
        format!("{} days ago", minutes / 60 / 24)
    }
}

impl SettingsView {
    fn reload_agents(&mut self, key: &str, cx: &mut Context<Self>) {
        self.agents.loaded = Some(key.to_owned());
        let (core, k) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.agents(&k).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => this.agents.list = Some(list),
                    Err(e) => {
                        this.agents.list = Some(Vec::new());
                        this.toast("circle-alert", e.message, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn agents_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        if self.agents.loaded.as_deref() != Some(key.as_str()) {
            self.agents = AgentsForm::default();
            self.reload_agents(&key, cx);
        }
        let (creation, admin) = self.core.shared.read(|s| {
            s.instance(&key)
                .map(|i| (i.node.as_ref().map(|n| n.agent_creation).unwrap_or(1), i.admin))
                .unwrap_or((1, false))
        });
        let everyone = pb::AgentCreation::Everyone as i32;
        let allowed = creation == everyone || creation == 0 || (creation == pb::AgentCreation::Admins as i32 && admin);
        let ready = self.agents.list.is_some();
        let making = self.agents.making;
        let header = div()
            .relative()
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(gpui_kit::linear_gradient(
                135.0,
                gpui_kit::linear_color_stop(alpha(rgb(VIOLET), 0.1), 0.0),
                gpui_kit::linear_color_stop(alpha(p.primary, 0.0), 1.0),
            ))
            .p(px(20.0))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(16.0))
                    .child(motion::ambient(
                        div()
                            .relative()
                            .size(px(48.0))
                            .flex_none()
                            .rounded(radius_2xl())
                            .bg(alpha(rgb(VIOLET), 0.15))
                            .text_color(rgb(VIOLET))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("bot").size(px(24.0)))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-4.0))
                                    .right(px(-4.0))
                                    .size(px(10.0))
                                    .rounded_full()
                                    .bg(rgb(0x34d399)),
                            ),
                        "agents-bot",
                        Duration::from_millis(4600),
                        window,
                        |el, t| {
                            let k = (t * 4600.0 / 3200.0).min(1.0);
                            el.top(px(-4.0 * (k * std::f32::consts::PI).sin()))
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(240.0))
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("settings.nav.agents")))
                            .child(
                                div().text_sm().text_color(p.muted_foreground).child(t("accountsettings.agents.intro")),
                            ),
                    )
                    .child(
                        button(
                            "agent-new",
                            t("accountsettings.agents.new"),
                            Some(if making { "x" } else { "plus" }),
                            Look::Primary,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .when(!allowed || !ready, |el| el.opacity(0.5))
                        .when(allowed && ready, |el| {
                            el.on_click(cx.listener(|this, _, window, cx| {
                                this.agents.making = !this.agents.making;
                                if this.agents.making {
                                    let name = cx.new(|cx| {
                                        InputState::new(window, cx)
                                            .placeholder(t("accountsettings.agents.namePlaceholder"))
                                    });
                                    cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()).detach();
                                    name.update(cx, |s, cx| s.focus(window, cx));
                                    let user = cx.new(|cx| InputState::new(window, cx).placeholder("helper_bot"));
                                    cx.subscribe(&user, |this: &mut SettingsView, _, e: &InputEvent, cx| {
                                        if matches!(e, InputEvent::Change) {
                                            this.agents.edited_username = true;
                                        }
                                        cx.notify()
                                    })
                                    .detach();
                                    this.agents.name = Some(name);
                                    this.agents.username = Some(user);
                                    this.agents.edited_username = false;
                                }
                                cx.notify();
                            }))
                        }),
                    ),
            )
            .when(!allowed, |el| {
                el.child(
                    div()
                        .relative()
                        .mt(px(12.0))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(if creation == pb::AgentCreation::Admins as i32 {
                            t("accountsettings.agents.adminsOnly")
                        } else {
                            t("accountsettings.agents.notAllowed")
                        }),
                )
            });
        let mut page = div().flex().flex_col().gap(px(20.0)).child(header);
        if making {
            page = page.child(self.new_agent(&key, p, window, cx));
        }
        match self.agents.list.clone() {
            None => {
                page = page.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(div().h(px(64.0)).rounded(radius_2xl()).bg(p.muted))
                        .child(div().h(px(64.0)).rounded(radius_2xl()).bg(p.muted)),
                )
            }
            Some(list) if list.is_empty() => {
                if !making {
                    page = page.child(motion::rise(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(8.0))
                            .py(px(32.0))
                            .text_center()
                            .child(motion::ambient(
                                div().text_size(px(36.0)).child("🤖"),
                                "agents-empty",
                                Duration::from_millis(3200),
                                window,
                                |el, t| {
                                    let k = (t * 3200.0 / 2400.0).min(1.0);
                                    el.relative().top(px(-6.0 * (k * std::f32::consts::PI).sin()))
                                },
                            ))
                            .child(div().font_weight(FontWeight::BOLD).child(t("accountsettings.agents.none")))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("accountsettings.agents.noneHint")),
                            ),
                        "agents-none",
                        Duration::ZERO,
                        8.0,
                    ));
                }
            }
            Some(list) => {
                let mut cards = div().flex().flex_col().gap(px(8.0));
                for a in &list {
                    cards = cards.child(self.agent_card(&key, a, p, window, cx));
                }
                page = page.child(cards);
            }
        }
        // How agents connect.
        let how = self.agents.how_to;
        let url = self.core.shared.read(|s| s.instance(&key).map(|i| i.url.clone())).unwrap_or_default();
        let host = if self.core.prefs().hides_personal() {
            "••••••".to_owned()
        } else {
            url.trim_start_matches("https://").trim_start_matches("http://").to_owned()
        };
        page = page.child(
            div()
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .id("agents-how")
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .p(px(12.0))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agents.how_to = !this.agents.how_to;
                            cx.notify();
                        }))
                        .child(icon("terminal").size(px(16.0)).text_color(p.primary))
                        .child(div().flex_1().child(t("accountsettings.agents.howTo")))
                        .child(icon(if how { "chevron-up" } else { "chevron-down" }).size(px(16.0)).text_color(p.muted_foreground)),
                )
                .when(how, |el| {
                    el.child(motion::rise(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .px(px(12.0))
                            .pb(px(12.0))
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(t_with(
                                "accountsettings.agents.howToApi",
                                &[("subscribe", Arg::Str("EventService/Subscribe")), ("send", Arg::Str("MessageService/SendMessage"))],
                            ))
                            .child(
                                div()
                                    .rounded(radius_xl())
                                    .bg(p.muted)
                                    .p(px(12.0))
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(p.foreground)
                                    .child(format!(
                                        "grpcurl -H 'authorization: Bearer <token>' \\\n  -d '{{\"server_id\": \"…\", \"channel_id\": \"…\", \"content\": \"Hello! 🤖\"}}' \\\n  {host}:443 fuwa.v1.MessageService/SendMessage"
                                    )),
                            )
                            .child(t("accountsettings.agents.howToRules")),
                        "agents-how-in",
                        Duration::ZERO,
                        -6.0,
                    ))
                }),
        );
        page.into_any_element()
    }

    fn new_agent(&mut self, key: &str, p: &Palette, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(name_state), Some(user_state)) = (self.agents.name.clone(), self.agents.username.clone()) else {
            return div().into_any_element();
        };
        let name = name_state.read(cx).value().to_string();
        let typed = user_state.read(cx).value().to_string();
        let username = if self.agents.edited_username { typed.to_lowercase() } else { suggest(&name) };
        let valid = valid_username(&username) && !name.trim().is_empty();
        let busy = self.agents.busy == Some("make");
        let label = |text: String| {
            div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(text.to_uppercase())
        };
        let k = key.to_owned();
        let shown = username.clone();
        let form = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(alpha(p.primary, 0.4))
            .bg(p.background)
            .p(px(16.0))
            .shadow(crate::ui::settings_controls::shadow_lg())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(192.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(label(t("accountsettings.agents.name")))
                            .child(
                                field(Input::new(&name_state).appearance(false), p)
                                    .h(px(40.0))
                                    .font_weight(FontWeight::BOLD),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(192.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(label(t("accountsettings.agents.username")))
                            .child(
                                field(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        .child(div().pl(px(11.0)).text_color(p.muted_foreground).child("@"))
                                        .child(div().flex_1().map(|el| {
                                            if self.agents.edited_username {
                                                el.child(Input::new(&user_state).appearance(false))
                                            } else {
                                                // Following the name until typed into: a click starts typing.
                                                el.child(
                                                    div()
                                                        .id("agent-username-follow")
                                                        .cursor_text()
                                                        .text_color(if shown.is_empty() {
                                                            p.muted_foreground
                                                        } else {
                                                            p.foreground
                                                        })
                                                        .on_click(cx.listener({
                                                            let shown = shown.clone();
                                                            move |this, _, window, cx| {
                                                                this.agents.edited_username = true;
                                                                if let Some(u) = this.agents.username.clone() {
                                                                    let shown = shown.clone();
                                                                    u.update(cx, |s, cx| {
                                                                        s.set_value(shown, window, cx);
                                                                        s.focus(window, cx);
                                                                    });
                                                                }
                                                                cx.notify();
                                                            }
                                                        }))
                                                        .child(if shown.is_empty() {
                                                            "helper_bot".to_owned()
                                                        } else {
                                                            shown.clone()
                                                        }),
                                                )
                                            }
                                        })),
                                    p,
                                )
                                .h(px(40.0))
                                .font_family("monospace")
                                .when(!username.is_empty() && !valid_username(&username), |el| {
                                    el.border_color(alpha(p.destructive, 0.6))
                                }),
                            ),
                    ),
            )
            .child(div().text_xs().text_color(p.muted_foreground).child(t("accountsettings.agents.usernameRule")))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        button("agent-cancel", t("common.cancel"), None, Look::Ghost, false, p)
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agents.making = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        button(
                            "agent-make",
                            t("accountsettings.agents.make"),
                            Some(if busy { "loader-circle" } else { "bot" }),
                            Look::Primary,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if busy {
                                return;
                            }
                            if !valid {
                                this.agents.shake = Some(std::time::Instant::now());
                                cx.notify();
                                return;
                            }
                            this.agents.busy = Some("make");
                            let (core, k, u, n) =
                                (this.core.clone(), k.clone(), username.clone(), name.trim().to_owned());
                            let rx = this.core.spawn(async move { core.create_agent(&k, &u, &n).await });
                            cx.spawn(async move |this, cx| {
                                let Ok(result) = rx.await else { return };
                                let _ = this.update(cx, |this, cx| {
                                    this.agents.busy = None;
                                    match result {
                                        Ok((agent, token)) => {
                                            let id = agent.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                                            this.agents.list.get_or_insert_with(Vec::new).push(agent);
                                            this.agents.making = false;
                                            this.agents.open = Some(id.clone());
                                            this.agents.fresh = Some((id, token));
                                            this.agents.token_shown = !this.core.prefs().hides_personal();
                                        }
                                        Err(e) => {
                                            this.toast("circle-alert", e.message, cx);
                                            this.agents.shake = Some(std::time::Instant::now());
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                            cx.notify();
                        })),
                    ),
            );
        let form: AnyElement = match self.agents.shake {
            Some(at) => motion::once(
                form,
                SharedString::from(format!("agent-shake-{at:?}")),
                Duration::from_millis(350),
                |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.0).sin() * 8.0 * (1.0 - t))),
            ),
            None => form.into_any_element(),
        };
        motion::rise(div().child(form), "agent-new-in", Duration::ZERO, -8.0).into_any_element()
    }

    fn save_agent(&mut self, key: &str, req: pb::UpdateAgentRequest, cx: &mut Context<Self>) {
        self.agents.busy = Some("save");
        let (core, k) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.update_agent(&k, req).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.agents.busy = None;
                match result {
                    Ok(agent) => {
                        if let Some(list) = this.agents.list.as_mut()
                            && let Some(slot) = list
                                .iter_mut()
                                .find(|x| x.user.as_ref().map(|u| &u.id) == agent.user.as_ref().map(|u| &u.id))
                        {
                            *slot = agent;
                        }
                    }
                    Err(e) => this.toast("circle-alert", e.message, cx),
                }
                this.agents.edit_for = None;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn agent_card(
        &mut self,
        key: &str,
        a: &pb::Agent,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(user) = a.user.clone() else { return div().into_any_element() };
        let open = self.agents.open.as_deref() == Some(user.id.as_str());
        let last = crate::ui::text::ms_of(a.last_active_at.as_ref());
        let recent = last > 0 && crate::core::dms::now_ms() - last < 10 * 60_000;
        let id = user.id.clone();
        let saving = self.agents.busy == Some("save") && open;
        let turn = motion::follow(
            SharedString::from(format!("agent-chevron-{}", user.id)),
            if open { 180.0 } else { 0.0 },
            window,
            cx,
        );
        let summary = div()
            .id(SharedString::from(format!("agent-{}", user.id)))
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.agents.open =
                    if this.agents.open.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                this.agents.confirm = None;
                cx.notify();
            }))
            // The avatar tips and grows a little under the pointer (`rotate: -8, scale: 1.08`).
            .child(
                div()
                    .id(SharedString::from(format!("agent-face-{}", user.id)))
                    .relative()
                    .hover(|s| s.rotate(gpui_kit::radians(-8f32.to_radians())).scale(1.08))
                    .child(avatar(Some(&user), 40.0, p))
                    .when(recent, |el| {
                        el.child(motion::pop(
                            div()
                                .absolute()
                                .right(px(-2.0))
                                .bottom(px(-2.0))
                                .size(px(12.0))
                                .rounded_full()
                                .border_2()
                                .border_color(p.background)
                                .bg(rgb(0x10b981)),
                            SharedString::from(format!("agent-recent-{}", user.id)),
                            0.05,
                            0.0,
                            Duration::ZERO,
                        ))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .child(crate::core::store::user_name(&user)),
                            )
                            .child(crate::ui::widgets::app_badge(
                                SharedString::from(format!("agent-badge-{}", user.id)),
                                "AGENT",
                                p,
                            ))
                            .when(a.public, |el| {
                                el.child(
                                    div()
                                        .rounded_full()
                                        .bg(p.muted)
                                        .px(px(6.0))
                                        .text_size(px(9.6))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(p.muted_foreground)
                                        .child(t("accountsettings.agents.public").to_uppercase()),
                                )
                            }),
                    )
                    .child(div().truncate().text_xs().text_color(p.muted_foreground).child(format!(
                        "@{} · {} · {}",
                        user.username,
                        t_with("accountsettings.agents.servers", &[("count", Arg::Num(i64::from(a.servers)))]),
                        if last > 0 {
                            t_with("accountsettings.agents.active", &[("when", Arg::Str(&ago(last)))])
                        } else {
                            t("accountsettings.agents.neverSignedIn")
                        }
                    ))),
            )
            .when(saving, |el| el.child(icon("loader-circle").size(px(16.0)).text_color(p.muted_foreground)))
            // The chevron turns over as the card opens (`transition-transform duration-300`).
            .child(
                div()
                    .rotate(gpui_kit::radians(turn.to_radians()))
                    .child(icon("chevron-down").size(px(16.0)).text_color(p.muted_foreground)),
            );
        let hover = alpha(p.primary, 0.3);
        let mut card = div()
            .id(SharedString::from(format!("agent-card-{}", user.id)))
            .overflow_hidden()
            .rounded(radius_2xl())
            .border_1()
            .border_color(if open { alpha(p.primary, 0.4) } else { p.border.into() })
            .bg(alpha(p.background, 0.5))
            .when(!open, |el| el.hover(move |s| s.border_color(hover)))
            .child(summary);
        if open {
            card = card.child(motion::rise(
                self.agent_body(key, a, &user, p, window, cx),
                SharedString::from(format!("agent-body-{}", user.id)),
                Duration::ZERO,
                -8.0,
            ));
        }
        motion::rise(card, SharedString::from(format!("agent-in-{}", user.id)), Duration::ZERO, 10.0).into_any_element()
    }

    fn agent_body(
        &mut self,
        key: &str,
        a: &pb::Agent,
        user: &pb::User,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        // The fields for the open agent, filled once.
        if self.agents.edit_for.as_deref() != Some(user.id.as_str()) {
            self.agents.edit_for = Some(user.id.clone());
            let (name, bio) = (user.display_name.clone(), a.bio.clone());
            let name_state = cx.new(|cx| InputState::new(window, cx));
            name_state.update(cx, |s, cx| s.set_value(name, window, cx));
            let (k, id) = (key.to_owned(), user.id.clone());
            cx.subscribe(&name_state, move |this: &mut SettingsView, s, e: &InputEvent, cx| match e {
                InputEvent::Blur | InputEvent::PressEnter { .. } => {
                    let v = s.read(cx).value().trim().to_owned();
                    let current = this
                        .agents
                        .list
                        .iter()
                        .flatten()
                        .find(|x| x.user.as_ref().is_some_and(|u| u.id == id))
                        .and_then(|x| x.user.clone())
                        .map(|u| u.display_name)
                        .unwrap_or_default();
                    if !v.is_empty() && v != current {
                        this.save_agent(
                            &k,
                            pb::UpdateAgentRequest {
                                agent_id: id.clone(),
                                display_name: Some(v),
                                ..Default::default()
                            },
                            cx,
                        );
                    }
                }
                _ => cx.notify(),
            })
            .detach();
            let bio_state = cx.new(|cx| {
                TextareaState::new(window, cx).auto_grow(3, 10).placeholder(t("accountsettings.agents.bioPlaceholder"))
            });
            bio_state.update(cx, |s, cx| s.set_value(bio, window, cx));
            let (k, id) = (key.to_owned(), user.id.clone());
            cx.subscribe(&bio_state, move |this: &mut SettingsView, s, e: &InputEvent, cx| match e {
                InputEvent::Blur => {
                    let v = s.read(cx).value().to_string();
                    let current = this
                        .agents
                        .list
                        .iter()
                        .flatten()
                        .find(|x| x.user.as_ref().is_some_and(|u| u.id == id))
                        .map(|x| x.bio.clone())
                        .unwrap_or_default();
                    if v != current {
                        this.save_agent(
                            &k,
                            pb::UpdateAgentRequest { agent_id: id.clone(), bio: Some(v), ..Default::default() },
                            cx,
                        );
                    }
                }
                _ => cx.notify(),
            })
            .detach();
            self.agents.edit_name = Some(name_state);
            self.agents.edit_bio = Some(bio_state);
        }
        let label = |text: String| {
            div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(text.to_uppercase())
        };
        let bio_len = self.agents.edit_bio.as_ref().map(|b| b.read(cx).value().chars().count()).unwrap_or(0);
        let mut body = div().flex().flex_col().gap(px(16.0)).border_t_1().border_color(p.border).p(px(16.0));
        if let Some((fresh_for, token)) = self.agents.fresh.clone()
            && fresh_for == user.id
        {
            body = body.child(self.token_reveal(&token, p, cx));
        }
        let fields = div().flex().flex_wrap().items_start().gap(px(16.0)).child(avatar(Some(user), 80.0, p)).child(
            div()
                .flex_1()
                .min_w(px(224.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .when_some(self.agents.edit_name.clone(), |el, s| {
                    el.child(
                        div().flex().flex_col().gap(px(4.0)).child(label(t("accountsettings.agents.name"))).child(
                            field(Input::new(&s).appearance(false), p).h(px(36.0)).font_weight(FontWeight::BOLD),
                        ),
                    )
                })
                .when_some(self.agents.edit_bio.clone(), |el, s| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div().flex().justify_between().child(label(t("accountsettings.agents.about"))).child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(if bio_len * 10 > 18000 { rgb(AMBER) } else { p.muted_foreground })
                                        .flex()
                                        .child(crate::ui::motion::rolling(
                                            "agent-bio-count",
                                            bio_len as u64,
                                            None,
                                            12.0,
                                        ))
                                        .child(" / 2000"),
                                ),
                            )
                            .child(
                                div()
                                    .min_h(px(80.0))
                                    .rounded(radius_xl())
                                    .border_1()
                                    .border_color(p.border)
                                    .bg(p.background)
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .text_sm()
                                    .child(Textarea::new(&s).appearance(false)),
                            ),
                    )
                }),
        );
        body = body.child(fields);
        let (k, id, public) = (key.to_owned(), user.id.clone(), a.public);
        body = body.child(toggle(
            crate::ui::settings_servers::leak_id(&format!("agent-public-{}", user.id)),
            &t("accountsettings.agents.public"),
            Some(&t("accountsettings.agents.publicHint")),
            public,
            false,
            p,
            window,
            cx,
            move |this, on, cx| {
                this.save_agent(
                    &k,
                    pb::UpdateAgentRequest { agent_id: id.clone(), public: Some(on), ..Default::default() },
                    cx,
                )
            },
        ));
        // Add to a server you manage.
        let managed: Vec<(pb::Server, bool)> = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return Vec::new() };
            i.servers
                .iter()
                .filter(|sv| i.access(&sv.id).has(pb::Permission::ManageServer))
                .map(|sv| {
                    (
                        sv.clone(),
                        i.members
                            .get(&sv.id)
                            .is_some_and(|m| m.iter().any(|m| m.user.as_ref().is_some_and(|u| u.id == user.id))),
                    )
                })
                .collect()
        });
        let mut items = vec![Item::Label(t("accountsettings.agents.managed"))];
        if managed.is_empty() {
            items.push(Item::Label(t("accountsettings.agents.noManaged")));
        }
        for (server, here) in managed {
            let (k, sid, username, uid) = (key.to_owned(), server.id.clone(), user.username.clone(), user.id.clone());
            items.push(Item::action(
                if here { format!("{} ✓", server.name) } else { server.name.clone() },
                Some("server"),
                move |this, cx| {
                    if here {
                        return;
                    }
                    this.agents.busy = Some("add");
                    let (core, k, sid, username, uid) =
                        (this.core.clone(), k.clone(), sid.clone(), username.clone(), uid.clone());
                    let rx = this.core.spawn(async move { core.add_agent(&k, &sid, &username).await });
                    cx.spawn(async move |this, cx| {
                        let Ok(result) = rx.await else { return };
                        let _ = this.update(cx, |this, cx| {
                            this.agents.busy = None;
                            match result {
                                Ok(_) => {
                                    this.agents.added = Some(uid.clone());
                                    if let Some(a) = this.agents.list.as_mut().and_then(|l| {
                                        l.iter_mut().find(|a| a.user.as_ref().is_some_and(|u| u.id == uid))
                                    }) {
                                        a.servers += 1;
                                    }
                                }
                                Err(e) => this.toast("circle-alert", e.message, cx),
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                },
            ));
        }
        let added = self.agents.added.as_deref() == Some(user.id.as_str());
        let busy = self.agents.busy;
        let add = self.dropdown(
            format!("agent-add-{}", user.id),
            button(
                SharedString::from(format!("agent-add-btn-{}", user.id)),
                if added { t("accountsettings.agents.added") } else { t("accountsettings.agents.addToServer") },
                Some(if added {
                    "check"
                } else if busy == Some("add") {
                    "loader-circle"
                } else {
                    "server"
                }),
                Look::Primary,
                true,
                p,
            )
            .rounded(radius_xl())
            .font_weight(FontWeight::BOLD),
            items,
            false,
            256.0,
            p,
            cx,
        );
        let created = crate::ui::text::ms_of(a.created_at.as_ref());
        let danger: AnyElement = match self.agents.confirm {
            Some(which) => {
                let (k, id) = (key.to_owned(), user.id.clone());
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .child(icon("triangle-alert").size(px(14.0)).text_color(rgb(AMBER)))
                            .child(if which == "reset" {
                                t("accountsettings.agents.resetWarning")
                            } else {
                                t("accountsettings.agents.deleteWarning")
                            }),
                    )
                    .child(
                        button(
                            SharedString::from(format!("agent-do-{}", user.id)),
                            if which == "reset" {
                                t("accountsettings.agents.newToken")
                            } else {
                                t("accountsettings.agents.delete")
                            },
                            None,
                            Look::Destructive,
                            true,
                            p,
                        )
                        .rounded_full()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.agents.confirm = None;
                            let (core, k, id) = (this.core.clone(), k.clone(), id.clone());
                            if which == "reset" {
                                this.agents.busy = Some("reset");
                                let rx = this.core.spawn(async move { core.reset_agent_token(&k, &id).await });
                                let id = this.agents.edit_for.clone().unwrap_or_default();
                                cx.spawn(async move |this, cx| {
                                    let Ok(result) = rx.await else { return };
                                    let _ = this.update(cx, |this, cx| {
                                        this.agents.busy = None;
                                        match result {
                                            Ok(token) => {
                                                this.agents.fresh = Some((id, token));
                                                this.agents.token_shown = !this.core.prefs().hides_personal();
                                            }
                                            Err(e) => this.toast("circle-alert", e.message, cx),
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                            } else {
                                let gone = id.clone();
                                let rx = this.core.spawn(async move { core.delete_agent(&k, &id).await });
                                cx.spawn(async move |this, cx| {
                                    let Ok(result) = rx.await else { return };
                                    let _ = this.update(cx, |this, cx| {
                                        match result {
                                            Ok(()) => {
                                                if let Some(l) = this.agents.list.as_mut() {
                                                    l.retain(|a| a.user.as_ref().is_none_or(|u| u.id != gone));
                                                }
                                            }
                                            Err(e) => this.toast("circle-alert", e.message, cx),
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        button(
                            SharedString::from(format!("agent-never-{}", user.id)),
                            "",
                            Some("x"),
                            Look::Ghost,
                            true,
                            p,
                        )
                        .w(px(32.0))
                        .px(px(0.0))
                        .rounded_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agents.confirm = None;
                            cx.notify();
                        })),
                    )
                    .into_any_element()
            }
            None => div()
                .flex()
                .gap(px(8.0))
                .child(
                    button(
                        SharedString::from(format!("agent-reset-{}", user.id)),
                        t("accountsettings.agents.newToken"),
                        Some("refresh-cw"),
                        Look::Ghost,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agents.confirm = Some("reset");
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        SharedString::from(format!("agent-delete-{}", user.id)),
                        t("accountsettings.agents.delete"),
                        Some("trash"),
                        Look::Ghost,
                        true,
                        p,
                    )
                    .rounded(radius_xl())
                    .text_color(p.destructive)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agents.confirm = Some("delete");
                        cx.notify();
                    })),
                )
                .into_any_element(),
        };
        body.child(
            div().flex().flex_wrap().items_center().gap(px(8.0)).child(add).child(danger).child(
                div()
                    .ml_auto()
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(t_with("accountsettings.agents.made", &[("when", Arg::Str(&ago(created)))])),
            ),
        )
    }

    /// A token on screen: shown once, copied, then put away. Streamer mode keeps it hidden.
    fn token_reveal(&mut self, token: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let shown = self.agents.token_shown;
        let copied = self.agents.copied;
        let value = token.to_owned();
        motion::rise(
            div()
                .relative()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_1()
                .border_color(alpha(rgb(AMBER), 0.4))
                .bg(alpha(rgb(AMBER), 0.1))
                .p(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(rgb(0xd97706))
                        .child(icon("key-round").size(px(16.0)))
                        .child(t("accountsettings.agents.copyNow")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .bg(alpha(p.background, 0.7))
                        .p(px(4.0))
                        .pl(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_family("monospace")
                                .when(!shown, |el| el.truncate())
                                .child(if shown { token.to_owned() } else { "•".repeat(32) }),
                        )
                        .child(
                            button("token-eye", "", Some(if shown { "eye-off" } else { "eye" }), Look::Ghost, true, p)
                                .w(px(32.0))
                                .px(px(0.0))
                                .rounded(radius_lg())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.agents.token_shown = !this.agents.token_shown;
                                    cx.notify();
                                })),
                        )
                        .child(
                            button(
                                "token-copy",
                                if copied {
                                    t("accountsettings.shared.copied")
                                } else {
                                    t("accountsettings.shared.copy")
                                },
                                Some(if copied { "check" } else { "copy" }),
                                Look::Primary,
                                true,
                                p,
                            )
                            .rounded(radius_lg())
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                                this.agents.copied = true;
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(Duration::from_millis(1400)).await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.agents.copied = false;
                                        cx.notify();
                                    });
                                })
                                .detach();
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .child(t("accountsettings.agents.tokenWarning")),
                        )
                        .child(
                            button("token-saved", t("accountsettings.shared.savedIt"), None, Look::Ghost, true, p)
                                .h(px(28.0))
                                .rounded(radius_lg())
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.agents.fresh = None;
                                    cx.notify();
                                })),
                        ),
                ),
            "token-in",
            Duration::ZERO,
            -8.0,
        )
        .into_any_element()
    }
}
