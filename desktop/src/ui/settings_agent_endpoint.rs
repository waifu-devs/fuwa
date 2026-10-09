//! An agent's endpoint on screen (docs/agent-endpoints.md), inside its open
//! card on the Agents page, as the web's `settings/account/AgentEndpoint.tsx`:
//! the URL the instance posts the agent's events to (checked by the instance
//! before it's saved), which events, the signing secret (dotted out, and
//! never shown in streamer mode) and how deliveries are going. Only where the
//! instance has `agent-endpoints`.

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::account_settings::{
    EVENT_NAMES, EndpointStatus, EventGroup, endpoint_status, grouped, offered, readable, same_events,
    toggle_all_events, toggle_event,
};
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{Look, Opt, button, choice, field};
use crate::ui::theme::{Palette, alpha, corner, radius_2xl, radius_lg, radius_xl};
use crate::ui::widgets::icon;

const VIOLET: u32 = 0x8b5cf6;
const AMBER: u32 = 0xf59e0b;
const EMERALD: u32 = 0x10b981;
const SKY: u32 = 0x0ea5e9;

/// The how-to lines, kept whole so their `{placeholders}` can be drawn as code.
const HOW_CHECK: &str = "accountsettings.agents.endpoint.howToCheck";
const HOW_VERIFY: &str = "accountsettings.agents.endpoint.howToVerify";
const HOW_REPLY: &str = "accountsettings.agents.endpoint.howToReply";

fn group_label(g: EventGroup) -> String {
    t(match g {
        EventGroup::Messages => "accountsettings.agents.endpoint.group.messages",
        EventGroup::Interactions => "accountsettings.agents.endpoint.group.interactions",
        EventGroup::Members => "accountsettings.agents.endpoint.group.members",
        EventGroup::Reactions => "accountsettings.agents.endpoint.group.reactions",
        EventGroup::Other => "accountsettings.agents.endpoint.group.other",
    })
}

/// Friendly names for the events most agents want; the rest read as their protocol name.
fn event_label(name: &str) -> String {
    let key = match name {
        "message_created" => "accountsettings.agents.endpoint.event.messageCreated",
        "message_updated" => "accountsettings.agents.endpoint.event.messageUpdated",
        "message_deleted" => "accountsettings.agents.endpoint.event.messageDeleted",
        "interaction_created" => "accountsettings.agents.endpoint.event.interactionCreated",
        "member_joined" => "accountsettings.agents.endpoint.event.memberJoined",
        "member_left" => "accountsettings.agents.endpoint.event.memberLeft",
        "member_updated" => "accountsettings.agents.endpoint.event.memberUpdated",
        "reaction_updated" => "accountsettings.agents.endpoint.event.reactionUpdated",
        "reactions_cleared" => "accountsettings.agents.endpoint.event.reactionsCleared",
        _ => return readable(name),
    };
    t(key)
}

#[derive(Default)]
pub(crate) struct EndpointForm {
    /// Whose endpoint this is.
    for_agent: Option<String>,
    /// As the instance last said; `None` while it's coming.
    endpoint: Option<pb::AgentEndpoint>,
    load_error: Option<String>,
    url: Option<Entity<InputState>>,
    /// "Only some" rather than all events, and which.
    some: bool,
    chosen: Vec<String>,
    /// Why the last save didn't go through, in the instance's words.
    error: Option<String>,
    busy: Option<&'static str>,
    /// When the last save went through, for the button's check.
    done: Option<Instant>,
    shake: Option<Instant>,
    secret_shown: bool,
    copied: bool,
    confirm_secret: bool,
    how_to: bool,
    /// A save turned it off: the URL field empties on the next draw (which has the window).
    url_cleared: bool,
}

impl EndpointForm {
    fn typed(&self, cx: &gpui_kit::App) -> String {
        self.url.as_ref().map(|u| u.read(cx).value().trim().to_owned()).unwrap_or_default()
    }

    /// The events a save sends: none for all of them.
    fn events(&self) -> Vec<String> {
        if self.some { self.chosen.clone() } else { Vec::new() }
    }

    fn needs_one(&self) -> bool {
        self.some && self.chosen.is_empty()
    }

    fn can_save(&self, cx: &gpui_kit::App) -> bool {
        let Some(e) = self.endpoint.as_ref() else { return false };
        let url = self.typed(cx);
        let dirty = url != e.url || !same_events(&self.events(), &e.events);
        !url.is_empty() && !self.needs_one() && (dirty || e.disabled_at.is_some()) && self.busy.is_none()
    }

    fn done(&self) -> bool {
        self.done.is_some_and(|at| at.elapsed() < Duration::from_millis(1600))
    }
}

impl SettingsView {
    /// Starts on an agent's endpoint: a fresh form, and the endpoint asked for.
    fn load_endpoint(&mut self, key: &str, agent_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.agents.endpoint = EndpointForm { for_agent: Some(agent_id.to_owned()), ..Default::default() };
        // Streamer mode hides the address, as the web's private fields do.
        let private = self.core.prefs().hides_personal();
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("https://example.com/fuwa").masked(private));
        let (k, id) = (key.to_owned(), agent_id.to_owned());
        cx.subscribe(&url, move |this: &mut SettingsView, _, e: &InputEvent, cx| {
            match e {
                InputEvent::PressEnter { .. } => this.save_endpoint(&k, &id, false, cx),
                InputEvent::Change => this.agents.endpoint.error = None,
                _ => {}
            }
            cx.notify();
        })
        .detach();
        self.agents.endpoint.url = Some(url);
        let (core, k, id) = (self.core.clone(), key.to_owned(), agent_id.to_owned());
        let rx = self.core.spawn(async move { core.agent_endpoint(&k, &id).await });
        let id = agent_id.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.agents.endpoint.for_agent.as_deref() != Some(id.as_str()) {
                    return;
                }
                match result {
                    Ok(endpoint) => this.take_endpoint(endpoint, Some(window), cx),
                    Err(e) => this.agents.endpoint.load_error = Some(e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The endpoint as the instance has it, filling the form (the URL too, given the window).
    fn take_endpoint(&mut self, endpoint: pb::AgentEndpoint, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let form = &mut self.agents.endpoint;
        if let (Some(url), Some(window)) = (form.url.clone(), window) {
            let value = endpoint.url.clone();
            url.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        form.some = !endpoint.events.is_empty();
        form.chosen = endpoint.events.clone();
        form.endpoint = Some(endpoint);
    }

    /// Saves the URL and events, or turns the endpoint off. The instance checks the URL first.
    fn save_endpoint(&mut self, key: &str, agent_id: &str, off: bool, cx: &mut Context<Self>) {
        let form = &self.agents.endpoint;
        let Some(saved) = form.endpoint.clone() else { return };
        if form.busy.is_some() {
            return;
        }
        if !off && !form.can_save(cx) {
            if form.typed(cx).is_empty() || form.needs_one() {
                self.agents.endpoint.shake = Some(Instant::now());
                cx.notify();
            }
            return;
        }
        let url = if off { String::new() } else { form.typed(cx) };
        let events = if off { saved.events.clone() } else { form.events() };
        self.agents.endpoint.busy = Some(if off { "off" } else { "save" });
        self.agents.endpoint.error = None;
        let (core, k, id) = (self.core.clone(), key.to_owned(), agent_id.to_owned());
        let rx = self.core.spawn(async move { core.set_agent_endpoint(&k, &id, &url, events).await });
        let id = agent_id.to_owned();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.agents.endpoint.for_agent.as_deref() != Some(id.as_str()) {
                    return;
                }
                this.agents.endpoint.busy = None;
                match result {
                    Ok(endpoint) => {
                        // The URL field already says what was saved; turning off empties it below.
                        let gone = endpoint.url.is_empty();
                        this.take_endpoint(endpoint, None, cx);
                        if off || gone {
                            this.agents.endpoint.url_cleared = true;
                            this.toast("power", t("accountsettings.agents.endpoint.turnedOff"), cx);
                        } else {
                            this.agents.endpoint.done = Some(Instant::now());
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_millis(1650)).await;
                                let _ = this.update(cx, |_, cx| cx.notify());
                            })
                            .detach();
                        }
                    }
                    Err(e) => {
                        this.agents.endpoint.error = Some(e.message);
                        this.agents.endpoint.shake = Some(Instant::now());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reset_endpoint_secret(&mut self, key: &str, agent_id: &str, cx: &mut Context<Self>) {
        let form = &mut self.agents.endpoint;
        form.confirm_secret = false;
        form.busy = Some("secret");
        let (core, k, id) = (self.core.clone(), key.to_owned(), agent_id.to_owned());
        let rx = self.core.spawn(async move { core.reset_agent_endpoint_secret(&k, &id).await });
        let id = agent_id.to_owned();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.agents.endpoint;
                if form.for_agent.as_deref() != Some(id.as_str()) {
                    return;
                }
                form.busy = None;
                match result {
                    // Only the secret is new: what's being typed stays, and a new secret starts hidden.
                    Ok(fresh) => {
                        form.secret_shown = false;
                        if let Some(e) = form.endpoint.as_mut() {
                            e.secret = fresh.secret;
                        }
                    }
                    Err(e) => this.toast("circle-alert", e.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The Endpoint section of an open agent's card, or nothing where the instance doesn't have them.
    pub(crate) fn agent_endpoint(
        &mut self,
        key: &str,
        agent_id: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.core.shared.read(|s| s.instance(key).is_some_and(|i| i.has("agent-endpoints"))) {
            return None;
        }
        if self.agents.endpoint.for_agent.as_deref() != Some(agent_id) {
            self.load_endpoint(key, agent_id, window, cx);
        }
        if std::mem::take(&mut self.agents.endpoint.url_cleared)
            && let Some(url) = self.agents.endpoint.url.clone()
        {
            url.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let head = div()
            .flex()
            .items_start()
            .gap(px(10.0))
            .child(
                div()
                    .size(px(32.0))
                    .flex_none()
                    .rounded(radius_xl())
                    .bg(alpha(rgb(VIOLET), 0.15))
                    .text_color(rgb(VIOLET))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("webhook").size(px(16.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("accountsettings.agents.endpoint.title")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(t("accountsettings.agents.endpoint.intro")),
                    ),
            );
        let section = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.background, 0.4))
            .p(px(12.0))
            .child(head);
        let form = &self.agents.endpoint;
        let body: AnyElement = if let Some(error) = form.load_error.clone() {
            div()
                .rounded(radius_xl())
                .bg(alpha(p.destructive, 0.1))
                .px(px(12.0))
                .py(px(8.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.destructive)
                .child(t_with("accountsettings.agents.endpoint.loadFailed", &[("error", Arg::Str(&error))]))
                .into_any_element()
        } else if let Some(endpoint) = form.endpoint.clone() {
            motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(status_line(&endpoint, agent_id, p, window))
                    .child(self.endpoint_url(key, agent_id, &endpoint, p, cx))
                    .child(self.endpoint_events(agent_id, &endpoint, p, window, cx))
                    .child(self.endpoint_secret(key, agent_id, &endpoint, p, cx))
                    .child(self.endpoint_how_to(agent_id, p, window, cx)),
                SharedString::from(format!("endpoint-in-{agent_id}")),
                Duration::ZERO,
                6.0,
            )
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(crate::ui::instance_home::shimmer(40.0, radius_xl(), p, window, 0))
                .child(crate::ui::instance_home::shimmer(64.0, radius_xl(), p, window, 1))
                .into_any_element()
        };
        Some(section.child(body).into_any_element())
    }

    fn endpoint_url(
        &mut self,
        key: &str,
        agent_id: &str,
        endpoint: &pb::AgentEndpoint,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let form = &self.agents.endpoint;
        let Some(url_state) = form.url.clone() else { return div().into_any_element() };
        let can_save = form.can_save(cx);
        let (busy, error, done, shake) = (form.busy, form.error.clone(), form.done(), form.shake);
        let (label, glyph) = if done {
            (t("accountsettings.agents.endpoint.saved"), "check")
        } else if busy == Some("save") {
            (t("accountsettings.agents.endpoint.checking"), "loader-circle")
        } else {
            (t("accountsettings.agents.endpoint.save"), "webhook")
        };
        let (k, id) = (key.to_owned(), agent_id.to_owned());
        let save = button(SharedString::from(format!("endpoint-save-{agent_id}")), "", None, Look::Primary, true, p)
            .h(px(36.0))
            .rounded(radius_xl())
            .font_weight(FontWeight::BOLD)
            .child(motion::rise(
                div().flex().items_center().gap(px(6.0)).child(icon(glyph).size(px(16.0))).child(label.clone()),
                SharedString::from(format!("endpoint-save-label-{agent_id}-{glyph}")),
                Duration::ZERO,
                10.0,
            ))
            .when(!can_save && !done, |el| el.opacity(0.5))
            .on_click(cx.listener(move |this, _, _, cx| this.save_endpoint(&k, &id, false, cx)));
        let (k, id) = (key.to_owned(), agent_id.to_owned());
        let turn_off = (!endpoint.url.is_empty()).then(|| {
            motion::pop(
                button(
                    SharedString::from(format!("endpoint-off-{agent_id}")),
                    t("accountsettings.shared.turnOff"),
                    Some(if busy == Some("off") { "loader-circle" } else { "power" }),
                    Look::Ghost,
                    true,
                    p,
                )
                .h(px(36.0))
                .rounded(radius_xl())
                .when(busy.is_some(), |el| el.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| this.save_endpoint(&k, &id, true, cx))),
                SharedString::from(format!("endpoint-off-in-{agent_id}")),
                0.1,
                0.0,
                Duration::ZERO,
            )
        });
        let row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(
                field(Input::new(&url_state).appearance(false), p)
                    .flex_1()
                    .min_w(px(224.0))
                    .h(px(36.0))
                    .text_xs()
                    .font_family("monospace")
                    .when(error.is_some(), |el| el.border_color(alpha(p.destructive, 0.6))),
            )
            .child(save)
            .children(turn_off);
        // The row shakes when a save can't go or the instance turned the URL down.
        let row: AnyElement = match shake {
            Some(at) => motion::once(
                row,
                SharedString::from(format!("endpoint-shake-{at:?}")),
                Duration::from_millis(350),
                |el, t| el.relative().left(px((t * std::f32::consts::TAU * 2.0).sin() * 8.0 * (1.0 - t))),
            ),
            None => row.into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(label_caps(&t("accountsettings.agents.endpoint.url"), p))
            .child(row)
            .when_some(error, |el, error| {
                el.child(motion::rise(
                    div()
                        .mt(px(4.0))
                        .flex()
                        .items_start()
                        .gap(px(6.0))
                        .rounded(radius_xl())
                        .bg(alpha(p.destructive, 0.1))
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.destructive)
                        .child(div().pt(px(1.0)).child(icon("triangle-alert").size(px(14.0))))
                        .child(div().flex_1().min_w_0().child(error.clone())),
                    SharedString::from(format!("endpoint-error-{agent_id}-{error}")),
                    Duration::ZERO,
                    -6.0,
                ))
            })
            .into_any_element()
    }

    fn endpoint_events(
        &mut self,
        agent_id: &str,
        endpoint: &pb::AgentEndpoint,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let form = &self.agents.endpoint;
        let (some, chosen, needs_one) = (form.some, form.chosen.clone(), form.needs_one());
        let width = (self.column - 88.0).max(320.0);
        let mode = choice(
            "endpoint-events",
            Some(usize::from(some)),
            vec![
                Opt::new(
                    t("accountsettings.agents.endpoint.allEvents"),
                    t("accountsettings.agents.endpoint.allEventsHint"),
                    "infinity",
                ),
                Opt::new(
                    t("accountsettings.agents.endpoint.chosenEvents"),
                    t("accountsettings.agents.endpoint.chosenEventsHint"),
                    "list-checks",
                ),
            ],
            width,
            p,
            window,
            cx,
            |this, n, cx| {
                this.agents.endpoint.some = n == 1;
                this.agents.endpoint.error = None;
                cx.notify();
            },
        );
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(label_caps(&t("accountsettings.agents.endpoint.events"), p))
            .child(mode);
        if some {
            let mut picker = div().flex().flex_col().gap(px(12.0)).pt(px(4.0));
            for (group, names) in grouped(&offered(&EVENT_NAMES, &endpoint.events)) {
                picker = picker.child(group_picker(agent_id, group, &names, &chosen, p, cx));
            }
            if needs_one {
                picker = picker.child(motion::rise(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(AMBER))
                        .child(t("accountsettings.agents.endpoint.pickOne")),
                    SharedString::from(format!("endpoint-pick-one-{agent_id}")),
                    Duration::ZERO,
                    -4.0,
                ));
            }
            col = col.child(motion::rise(
                picker,
                SharedString::from(format!("endpoint-picker-{agent_id}")),
                Duration::ZERO,
                -8.0,
            ));
        }
        col.into_any_element()
    }

    fn endpoint_secret(
        &mut self,
        key: &str,
        agent_id: &str,
        endpoint: &pb::AgentEndpoint,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Secrets stay dotted out whenever streamer mode is on, with no way to show them.
        let streaming = self.core.prefs().streamer_mode;
        let form = &self.agents.endpoint;
        let visible = form.secret_shown && !streaming;
        let (copied, confirm, busy) = (form.copied, form.confirm_secret, form.busy);
        let value = endpoint.secret.clone();
        let shown = if visible { value.clone() } else { format!("whsec_{}", "•".repeat(28)) };
        let row = div()
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
                div().flex_1().min_w_0().text_xs().font_family("monospace").when(!visible, |el| el.truncate()).child(
                    motion::fade_in(
                        div().child(shown),
                        SharedString::from(format!("endpoint-secret-{agent_id}-{visible}-{}", value.len())),
                        Duration::from_millis(180),
                    ),
                ),
            )
            .child(
                button(
                    SharedString::from(format!("endpoint-eye-{agent_id}")),
                    "",
                    Some(if visible { "eye-off" } else { "eye" }),
                    Look::Ghost,
                    true,
                    p,
                )
                .w(px(32.0))
                .px(px(0.0))
                .rounded(radius_lg())
                .when(streaming, |el| el.opacity(0.4))
                .when(!streaming, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        this.agents.endpoint.secret_shown = !this.agents.endpoint.secret_shown;
                        cx.notify();
                    }))
                }),
            )
            .child(
                button(SharedString::from(format!("endpoint-copy-{agent_id}")), "", None, Look::Outline, true, p)
                    .rounded(radius_lg())
                    .font_weight(FontWeight::BOLD)
                    .child(motion::rise(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(icon(if copied { "check" } else { "copy" }).size(px(14.0)))
                            .child(if copied {
                                t("accountsettings.shared.copied")
                            } else {
                                t("accountsettings.shared.copy")
                            }),
                        SharedString::from(format!("endpoint-copy-label-{agent_id}-{copied}")),
                        Duration::ZERO,
                        12.0,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                        this.agents.endpoint.copied = true;
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(1400)).await;
                            let _ = this.update(cx, |this, cx| {
                                this.agents.endpoint.copied = false;
                                cx.notify();
                            });
                        })
                        .detach();
                        cx.notify();
                    })),
            );
        let (k, id) = (key.to_owned(), agent_id.to_owned());
        let renew: AnyElement = if confirm {
            motion::slide_in(
                div()
                    .flex()
                    .flex_wrap()
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
                            .child(t("accountsettings.agents.endpoint.newSecretWarning")),
                    )
                    .child(
                        button(
                            SharedString::from(format!("endpoint-secret-do-{agent_id}")),
                            t("accountsettings.agents.endpoint.newSecret"),
                            None,
                            Look::Destructive,
                            true,
                            p,
                        )
                        .h(px(28.0))
                        .rounded_full()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(move |this, _, _, cx| this.reset_endpoint_secret(&k, &id, cx))),
                    )
                    .child(
                        button(
                            SharedString::from(format!("endpoint-secret-no-{agent_id}")),
                            "",
                            Some("x"),
                            Look::Ghost,
                            true,
                            p,
                        )
                        .size(px(28.0))
                        .px(px(0.0))
                        .rounded_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agents.endpoint.confirm_secret = false;
                            cx.notify();
                        })),
                    ),
                SharedString::from(format!("endpoint-secret-confirm-{agent_id}")),
                8.0,
            )
            .into_any_element()
        } else {
            motion::slide_in(
                div().flex().items_center().gap(px(8.0)).child(
                    button(
                        SharedString::from(format!("endpoint-secret-new-{agent_id}")),
                        t("accountsettings.agents.endpoint.newSecret"),
                        Some(if busy == Some("secret") { "loader-circle" } else { "refresh-cw" }),
                        Look::Ghost,
                        true,
                        p,
                    )
                    .h(px(28.0))
                    .rounded(radius_xl())
                    .text_xs()
                    .when(busy.is_some(), |el| el.opacity(0.5))
                    .when(busy.is_none(), |el| {
                        el.on_click(cx.listener(|this, _, _, cx| {
                            this.agents.endpoint.confirm_secret = true;
                            cx.notify();
                        }))
                    }),
                ),
                SharedString::from(format!("endpoint-secret-action-{agent_id}")),
                -8.0,
            )
            .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_color(p.muted_foreground)
                    .child(icon("key-round").size(px(14.0)))
                    .child(label_caps(&t("accountsettings.agents.endpoint.secret"), p)),
            )
            .child(row)
            .when(streaming, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(t("accountsettings.agents.endpoint.streamerHidden")),
                )
            })
            .child(renew)
            .into_any_element()
    }

    /// What an endpoint has to do: answer the check, check signatures, reply in the body.
    fn endpoint_how_to(
        &mut self,
        agent_id: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.agents.endpoint.how_to;
        let turn = motion::follow(
            SharedString::from(format!("endpoint-how-chevron-{agent_id}")),
            if open { 180.0 } else { 0.0 },
            window,
            cx,
        );
        let line = |key: &str, values: &[(&str, &str)]| {
            div().flex().gap(px(6.0)).child(div().flex_none().child("•")).child(
                div().flex_1().min_w_0().child(crate::ui::text::hint_line(
                    &crate::ui::instance_home::template_text(key),
                    values,
                    p,
                )),
            )
        };
        div()
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .child(
                div()
                    .id(SharedString::from(format!("endpoint-how-{agent_id}")))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agents.endpoint.how_to = !this.agents.endpoint.how_to;
                        cx.notify();
                    }))
                    .child(icon("webhook").size(px(14.0)).text_color(p.primary))
                    .child(div().flex_1().child(t("accountsettings.agents.endpoint.howTo")))
                    .child(
                        div()
                            .rotate(gpui_kit::radians(turn.to_radians()))
                            .child(icon("chevron-down").size(px(14.0)).text_color(p.muted_foreground)),
                    ),
            )
            .when(open, |el| {
                el.child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(12.0))
                        .pb(px(12.0))
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(line(HOW_CHECK, &[("challenge", "challenge"), ("answer", "{\"challenge\": \"…\"}")]))
                        .child(line(HOW_VERIFY, &[("signature", "webhook-signature")]))
                        .child(line(
                            HOW_REPLY,
                            &[("replies", "{\"replies\": [{\"interactionId\": \"…\", \"content\": \"…\"}]}")],
                        )),
                    SharedString::from(format!("endpoint-how-in-{agent_id}")),
                    Duration::ZERO,
                    -6.0,
                ))
            })
            .into_any_element()
    }
}

/// The small uppercase label over each part (`text-xs font-bold uppercase`).
fn label_caps(text: &str, p: &Palette) -> gpui_kit::Div {
    div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child(text.to_uppercase())
}

/// How deliveries are going, with a dot that says it at a glance.
fn status_line(e: &pb::AgentEndpoint, agent_id: &str, p: &Palette, window: &Window) -> AnyElement {
    let now = crate::core::dms::now_ms();
    let status = endpoint_status(e);
    let (kind, text, dot): (&str, String, Hsla) = match &status {
        EndpointStatus::Disabled(_) => {
            ("disabled", t("accountsettings.agents.endpoint.statusDisabled"), p.destructive.into())
        }
        EndpointStatus::Off => ("off", t("accountsettings.agents.endpoint.statusOff"), alpha(p.muted_foreground, 0.5)),
        EndpointStatus::Failing(since, error) => (
            "failing",
            t_with(
                "accountsettings.agents.endpoint.statusFailing",
                &[("when", Arg::Str(&crate::ui::text::ago(*since, now))), ("error", Arg::Str(error))],
            ),
            rgb(AMBER).into(),
        ),
        EndpointStatus::Delivered(at) => (
            "delivered",
            t_with(
                "accountsettings.agents.endpoint.statusDelivered",
                &[("when", Arg::Str(&crate::ui::text::ago(*at, now)))],
            ),
            rgb(EMERALD).into(),
        ),
        EndpointStatus::Waiting => ("waiting", t("accountsettings.agents.endpoint.statusWaiting"), rgb(SKY).into()),
    };
    let (bg, fg): (Hsla, Hsla) = match kind {
        "disabled" => (alpha(p.destructive, 0.1), p.destructive.into()),
        "failing" => (alpha(rgb(AMBER), 0.1), rgb(0xb45309).into()),
        _ => (alpha(p.muted, 0.6), p.muted_foreground.into()),
    };
    let live = matches!(kind, "delivered" | "failing");
    div()
        .flex()
        .items_start()
        .gap(px(8.0))
        .rounded(radius_xl())
        .bg(bg)
        .px(px(12.0))
        .py(px(8.0))
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(fg)
        .child(
            div()
                .relative()
                .mt(px(4.0))
                .size(px(8.0))
                .flex_none()
                // A ring breathes out of the dot while deliveries are going (or failing).
                .when(live, |el| {
                    el.child(motion::ambient(
                        div().absolute().inset_0().rounded_full().bg(dot),
                        SharedString::from(format!("endpoint-pulse-{agent_id}")),
                        Duration::from_secs(2),
                        window,
                        move |el, t| {
                            let k = (t * std::f32::consts::PI).sin();
                            el.opacity(0.6 * (1.0 - k))
                                .size(px(8.0 * (1.0 + 1.4 * k)))
                                .left(px(-5.6 * k))
                                .top(px(-5.6 * k))
                        },
                    ))
                })
                .child(motion::pop(
                    div().absolute().inset_0().rounded_full().bg(dot),
                    SharedString::from(format!("endpoint-dot-{agent_id}-{kind}")),
                    0.05,
                    0.0,
                    Duration::ZERO,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(motion::swapping(SharedString::from(format!("endpoint-status-{agent_id}")), text, 12.0))
                .when_some(
                    match &status {
                        EndpointStatus::Disabled(error) if !error.is_empty() => Some(error.clone()),
                        _ => None,
                    },
                    |el, error| {
                        el.child(
                            div().font_weight(FontWeight::NORMAL).opacity(0.8).child(t_with(
                                "accountsettings.agents.endpoint.lastError",
                                &[("error", Arg::Str(&error))],
                            )),
                        )
                    },
                ),
        )
        .into_any_element()
}

/// One group of events: its name (which picks them all, with a count), and a chip per event.
fn group_picker(
    agent_id: &str,
    group: EventGroup,
    names: &[String],
    chosen: &[String],
    p: &Palette,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let picked = names.iter().filter(|n| chosen.contains(n)).count();
    let all = picked == names.len();
    let group_names = names.to_vec();
    let fg = p.foreground;
    let title = div()
        .id(SharedString::from(format!("endpoint-group-{agent_id}-{group:?}")))
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .cursor_pointer()
        .hover(move |s| s.text_color(fg))
        .on_click(cx.listener(move |this, _, _, cx| {
            let form = &mut this.agents.endpoint;
            form.chosen = toggle_all_events(&form.chosen, &group_names);
            cx.notify();
        }))
        .child(
            div()
                .size(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(6.0))
                .border_1()
                .map(|el| {
                    if all {
                        el.border_color(p.primary).bg(p.primary).text_color(p.primary_foreground)
                    } else if picked > 0 {
                        el.border_color(alpha(p.primary, 0.6)).bg(alpha(p.primary, 0.2))
                    } else {
                        el.border_color(p.border)
                    }
                })
                .when(all, |el| {
                    el.child(motion::pop(
                        icon("check").size(px(12.0)),
                        SharedString::from(format!("endpoint-group-all-{agent_id}-{group:?}")),
                        0.05,
                        0.0,
                        Duration::ZERO,
                    ))
                }),
        )
        .child(group_label(group))
        .child(
            div()
                .flex()
                .opacity(0.7)
                .child(motion::rolling(format!("endpoint-group-count-{agent_id}-{group:?}"), picked as u64, None, 12.0))
                .child(format!("/{}", names.len())),
        );
    let mut chips = div().flex().flex_wrap().gap(px(6.0));
    for name in names {
        let on = chosen.contains(name);
        let hover = alpha(p.primary, 0.3);
        let n = name.clone();
        chips = chips.child(
            div()
                .id(SharedString::from(format!("endpoint-event-{agent_id}-{name}")))
                .relative()
                .flex()
                .items_center()
                .gap(px(4.0))
                .rounded_full()
                .border_1()
                .px(px(10.0))
                .py(px(4.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .map(|el| {
                    if on {
                        el.border_color(alpha(p.primary, 0.6)).bg(alpha(p.primary, 0.15)).text_color(p.foreground)
                    } else {
                        el.border_color(p.border).text_color(p.muted_foreground).hover(move |s| s.border_color(hover))
                    }
                })
                .hover(|s| s.top(px(-1.0)))
                .active(|s| s.scale(0.94))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let form = &mut this.agents.endpoint;
                    form.chosen = toggle_event(&form.chosen, &n);
                    cx.notify();
                }))
                .when(on, |el| {
                    el.child(motion::pop(
                        icon("check").size(px(12.0)),
                        SharedString::from(format!("endpoint-event-on-{agent_id}-{name}")),
                        0.05,
                        0.0,
                        Duration::ZERO,
                    ))
                })
                .child(event_label(name)),
        );
    }
    div().flex().flex_col().gap(px(6.0)).child(title).child(chips).into_any_element()
}
