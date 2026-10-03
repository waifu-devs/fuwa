//! An instance's settings, for its admins, full screen like a server's:
//! the anonymous usage signal and the moderation services servers' AutoMod
//! can ask, as in the web app's `InstanceSettingsDialog.tsx`. Each setting
//! starts from the operator's environment; what's changed here is stored on
//! the instance, and "Back to the default" puts it back.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    hsla, px,
};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::instance_admin::{self as admin, MAX_CUSTOM};
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::{amber, chip, pill, save_bar, shimmer_rows, spinner, switch};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{error_line, icon, pal, soft_button};

pub enum InstanceSettingsEvent {
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Privacy,
    Moderation,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::Privacy => "Privacy",
            Page::Moderation => "Moderation",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Page::Privacy => "shield-check",
            Page::Moderation => "shield-alert",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Page::Privacy => "What this instance tells Waifu Devs.",
            Page::Moderation => "Services servers' AutoMod can ask about messages.",
        }
    }
}

/// The instance group, in the web's order.
const PAGES: [Page; 2] = [Page::Privacy, Page::Moderation];

/// What the usage signal counts, as on the web.
const SIGNAL: [&str; 5] = [
    "How many accounts, servers, channels and messages",
    "Storage used, in bytes",
    "Which account and server options are on",
    "fuwa version, OS and a random install id",
    "Kinds of errors and where, and how long requests took (server and apps)",
];

/// The text boxes of one provider's card, kept by a slot that doesn't move
/// when a card above it is removed.
struct Fields {
    slot: u64,
    key: Entity<InputState>,
    account: Entity<InputState>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    header: Entity<InputState>,
    model: Entity<InputState>,
}

/// A provider's "Try a sample scam".
enum Tried {
    Asking,
    Answered(pb::TestAutoModProviderResponse),
    Failed(String),
}

pub struct InstanceSettingsView {
    core: Arc<Core>,
    pub key: String,
    page: Page,
    config: Option<pb::InstanceConfig>,
    draft: Option<pb::InstanceSettings>,
    load_error: Option<String>,
    saving: bool,
    error: Option<String>,
    fields: Vec<Fields>,
    next_slot: u64,
    tried: HashMap<u64, Tried>,
    bar: Option<AnyElement>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InstanceSettingsEvent> for InstanceSettingsView {}

impl InstanceSettingsView {
    pub fn new(core: Arc<Core>, key: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // It reads whether you're still an admin as it draws.
        let mut changes = core.changes();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut view = Self {
            core,
            key,
            page: Page::Privacy,
            config: None,
            draft: None,
            load_error: None,
            saving: false,
            error: None,
            fields: Vec::new(),
            next_slot: 0,
            tried: HashMap::new(),
            bar: None,
            _subscriptions: Vec::new(),
        };
        view.load(window, cx);
        view
    }

    /// Runs `future` on the core and hands its result back with the window.
    fn run<T: Send + 'static>(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(value) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| done(this, value, window, cx));
            }
        })
        .detach();
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.instance_settings(&key).await }, |this, result, window, cx| {
            match result {
                Ok(config) => {
                    this.draft = config.settings.clone();
                    this.config = Some(config);
                    this.refill(window, cx);
                }
                Err(problem) => this.load_error = Some(problem.message),
            }
            cx.notify();
        });
    }

    fn saved(&self) -> Option<&pb::InstanceSettings> {
        self.config.as_ref().and_then(|c| c.settings.as_ref())
    }

    fn changed(&self) -> Vec<String> {
        match (&self.draft, self.saved()) {
            (Some(draft), Some(saved)) => admin::changed(draft, saved),
            _ => Vec::new(),
        }
    }

    fn overridden(&self, path: &str) -> bool {
        self.config.as_ref().is_some_and(|c| c.overridden.iter().any(|p| p == path))
    }

    fn patch(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut pb::InstanceSettings)) {
        if let Some(draft) = self.draft.as_mut() {
            f(draft);
        }
        self.error = None;
        cx.notify();
    }

    /// Saves `update` and resets `reset`, keeping edits to anything else.
    fn commit(&mut self, update: Vec<String>, reset: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else { return };
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        let (u, r) = (update.clone(), reset.clone());
        let pending: Vec<String> =
            self.changed().into_iter().filter(|p| !update.contains(p) && !reset.contains(p)).collect();
        self.run(
            window,
            cx,
            async move { core.update_instance_settings(&key, draft, u, r).await },
            move |this, result: Result<pb::InstanceConfig, Problem>, window, cx| {
                this.saving = false;
                match result {
                    Ok(config) => {
                        let mut fresh = config.settings.clone().unwrap_or_default();
                        if let Some(draft) = &this.draft {
                            if pending.iter().any(|p| p == "telemetry") {
                                fresh.telemetry = draft.telemetry;
                            }
                            if pending.iter().any(|p| p == "automod_providers") {
                                fresh.automod_providers = draft.automod_providers.clone();
                            }
                        }
                        let providers_from_saved = !pending.iter().any(|p| p == "automod_providers");
                        this.draft = Some(fresh);
                        this.config = Some(config);
                        if providers_from_saved {
                            // Saved keys come back as hints, and new ones get their ids.
                            this.refill(window, cx);
                        }
                    }
                    Err(problem) => this.error = Some(problem.message),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = self.saved().cloned();
        self.error = None;
        self.refill(window, cx);
        cx.notify();
    }

    /// Text boxes for each provider in the draft, filled from it.
    fn refill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fields.clear();
        self.tried.clear();
        self._subscriptions.clear();
        let providers = self.draft.as_ref().map(|d| d.automod_providers.clone()).unwrap_or_default();
        for provider in &providers {
            let fields = self.make_fields(provider, window, cx);
            self.fields.push(fields);
        }
    }

    fn make_fields(&mut self, p: &pb::AutoModProviderSettings, window: &mut Window, cx: &mut Context<Self>) -> Fields {
        self.next_slot += 1;
        let slot = self.next_slot;
        let input = |value: &str, placeholder: &str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            let (value, placeholder) = (value.to_owned(), placeholder.to_owned());
            cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(placeholder);
                if masked {
                    s = s.masked(true);
                }
                s.set_value(value, window, cx);
                s
            })
        };
        // Streamer mode keeps addresses and account ids off the screen, as on the web.
        let hide = self.core.prefs().streamer_mode;
        let fields = Fields {
            slot,
            key: input(&p.api_key, "Paste it here", true, window, cx),
            account: input(&p.account_id, "32 letters and digits", hide, window, cx),
            name: input(&p.name, "What servers see, like Our classifier", false, window, cx),
            url: input(&p.url, "https://moderation.example.com/v1/check", hide, window, cx),
            header: input(&p.header, "Authorization (Bearer)", false, window, cx),
            model: input(&p.model, "Optional, sent as model", false, window, cx),
        };
        type Set = fn(&mut pb::AutoModProviderSettings, String);
        let wires: [(&Entity<InputState>, Set); 6] = [
            (&fields.key, |p, v| p.api_key = v),
            (&fields.account, |p, v| p.account_id = v.trim().to_owned()),
            (&fields.name, |p, v| p.name = v.chars().take(40).collect()),
            (&fields.url, |p, v| p.url = v.chars().take(512).collect()),
            (&fields.header, |p, v| p.header = v.trim().chars().take(64).collect()),
            (&fields.model, |p, v| p.model = v.chars().take(100).collect()),
        ];
        for (state, set) in wires {
            let sub = cx.subscribe(state, move |this: &mut Self, state, e: &InputEvent, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                let value = state.read(cx).value().to_string();
                let Some(n) = this.fields.iter().position(|f| f.slot == slot) else { return };
                let differs = this.draft.as_ref().and_then(|d| d.automod_providers.get(n)).is_some_and(|p| {
                    let mut next = p.clone();
                    set(&mut next, value.clone());
                    next != *p
                });
                if differs {
                    this.patch(cx, |d| {
                        if let Some(p) = d.automod_providers.get_mut(n) {
                            set(p, value)
                        }
                    });
                    this.tried.remove(&slot);
                }
            });
            self._subscriptions.push(sub);
        }
        fields
    }

    fn add_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let customs =
            self.draft.as_ref().map_or(0, |d| d.automod_providers.iter().filter(|p| admin::is_custom(p)).count());
        if customs >= MAX_CUSTOM {
            return;
        }
        let fresh = pb::AutoModProviderSettings { id: "custom".into(), ..Default::default() };
        let fields = self.make_fields(&fresh, window, cx);
        fields.name.update(cx, |s, cx| s.focus(window, cx));
        self.fields.push(fields);
        self.patch(cx, |d| d.automod_providers.push(fresh));
    }

    fn remove(&mut self, slot: u64, cx: &mut Context<Self>) {
        let Some(n) = self.fields.iter().position(|f| f.slot == slot) else { return };
        self.fields.remove(n);
        self.tried.remove(&slot);
        self.patch(cx, |d| {
            if n < d.automod_providers.len() {
                d.automod_providers.remove(n);
            }
        });
    }

    fn try_provider(&mut self, slot: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(n) = self.fields.iter().position(|f| f.slot == slot) else { return };
        let Some(provider) = self.draft.as_ref().and_then(|d| d.automod_providers.get(n)).cloned() else { return };
        self.tried.insert(slot, Tried::Asking);
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(
            window,
            cx,
            async move { core.test_automod_provider(&key, provider).await },
            move |this, result, _, cx| {
                if this.tried.contains_key(&slot) {
                    this.tried.insert(
                        slot,
                        match result {
                            Ok(answer) => Tried::Answered(answer),
                            Err(problem) => Tried::Failed(problem.message),
                        },
                    );
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        self.error = None;
        cx.notify();
    }

    /// "Changed" and the way back to the default, for settings stored on the instance.
    fn reset_badge(&self, path: &'static str, default: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if !self.overridden(path) {
            return div().into_any_element();
        }
        let saving = self.saving;
        motion::rise(
            div().flex_none().flex().items_center().gap(px(6.0)).child(pill("CHANGED", p.primary.into())).child(
                div()
                    .id(SharedString::from(format!("instance-reset-{path}")))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .h(px(26.0))
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover({
                        let (bg, fg) = (alpha(p.primary, 0.1), p.foreground);
                        move |s| s.bg(bg).text_color(fg)
                    })
                    .when(saving, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.commit(Vec::new(), vec![path.to_owned()], window, cx)
                    }))
                    .child(icon("rotate-ccw").size(px(13.0)))
                    .child(format!("Back to the default: {default}")),
            ),
            SharedString::from(format!("instance-changed-{path}")),
            Duration::ZERO,
            -4.0,
        )
        .into_any_element()
    }

    fn privacy_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(defaults)) =
            (self.draft.as_ref(), self.config.as_ref().and_then(|c| c.defaults.as_ref()))
        else {
            return div().into_any_element();
        };
        let on = draft.telemetry;
        let default = if defaults.telemetry { "on" } else { "off" };
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (n, line) in SIGNAL.iter().enumerate() {
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(icon("shield-check").size(px(14.0)).text_color(p.primary))
                    .child(*line),
                SharedString::from(format!("signal-line-{n}")),
                Duration::from_millis(100 + 50 * n as u64),
                0.0,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(
                        div().flex_1().font_weight(FontWeight::EXTRA_BOLD).child("Anonymous usage signal and reports"),
                    )
                    .child(self.reset_badge("telemetry", default, p, cx)),
            )
            .child(
                div()
                    .id("instance-telemetry")
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(14.0))
                    .rounded(corner(16.0))
                    .border_1()
                    .cursor_pointer()
                    .map(|el| {
                        if on {
                            el.border_color(alpha(p.primary, 0.4)).bg(alpha(p.primary, 0.04))
                        } else {
                            el.border_color(p.border)
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.patch(cx, |d| d.telemetry = !on)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Send the usage signal daily and error reports hourly"),
                            )
                            .child(div().text_xs().text_color(p.muted_foreground).child(
                                "Helps Waifu Devs see how fuwa is used and fix what breaks. Counts only: no names, \
                                 messages, ids or addresses. Off, apps on this instance send no reports either.",
                            )),
                    )
                    .child(switch("instance-telemetry-switch".into(), on, false, cx, |this, on, cx| {
                        this.patch(cx, |d| d.telemetry = on)
                    })),
            )
            .child(list)
            .child(
                div()
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child("Every field it sends is listed in fuwa's README, under \"The anonymous usage signal\"."),
            )
            .into_any_element()
    }

    fn moderation_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = self.draft.clone() else { return div().into_any_element() };
        let violet = hsla(0.76, 0.7, if p.dark { 0.7 } else { 0.5 }, 1.0);
        let intro = motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(violet.opacity(0.3))
                .bg(violet.opacity(0.05))
                .child(motion::ambient(
                    div()
                        .flex_none()
                        .size(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(corner(11.0))
                        .bg(violet.opacity(0.15))
                        .text_color(violet)
                        .child(icon("sparkles").size(px(16.0))),
                    "instance-moderation-sparkle",
                    Duration::from_millis(6400),
                    window,
                    |el, t| {
                        // A little wiggle every few seconds, as on the web.
                        let k = (t * 6400.0 / 1400.0).min(1.0);
                        let bump = if k < 1.0 { (k * std::f32::consts::TAU).sin() } else { 0.0 };
                        el.relative().top(px(-2.0 * bump.abs()))
                    },
                ))
                .child(div().flex_1().min_w_0().text_sm().text_color(p.muted_foreground).child(
                    "Turn a service on here and every server on this instance can pick it for AutoMod's smart filter, \
                     with one switch. Only this instance talks to it, and only about messages in servers that turned \
                     the filter on: their text, never who wrote them, where, or the server's name. If it's slow or \
                     down, messages go through and the servers' own rules still apply.",
                )),
            "instance-moderation-intro",
            Duration::ZERO,
            8.0,
        );
        // What the operator's environment turns on, which "Back to the default" returns to.
        let on: Vec<String> = self
            .config
            .as_ref()
            .and_then(|c| c.defaults.as_ref())
            .map(|d| {
                d.automod_providers
                    .iter()
                    .filter(|x| x.enabled)
                    .map(|x| admin::known(&x.id).map_or_else(|| x.name.clone(), |k| k.name.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let providers_default = if on.is_empty() { "all off".to_owned() } else { format!("{} on", on.join(" and ")) };
        let customs = draft.automod_providers.iter().filter(|x| admin::is_custom(x)).count();
        let mut cards = div().flex().flex_col().gap(px(14.0));
        for (n, provider) in draft.automod_providers.iter().enumerate() {
            let Some(slot) = self.fields.get(n).map(|f| f.slot) else { continue };
            let saved = self.saved().and_then(|s| s.automod_providers.iter().find(|x| x.id == provider.id)).cloned();
            cards = cards.child(motion::rise(
                div().child(self.provider_card(n, slot, provider, saved.as_ref(), p, window, cx)),
                SharedString::from(format!("instance-provider-{slot}")),
                Duration::from_millis(40 + 50 * n.min(6) as u64),
                10.0,
            ));
        }
        let full = customs >= MAX_CUSTOM;
        let violet_bg = violet.opacity(0.15);
        let add = div()
            .id("instance-add-provider")
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(corner(22.0))
            .border_2()
            .border_dashed()
            .border_color(p.border)
            .when(full, |el| el.opacity(0.5))
            .when(!full, |el| {
                let (hover_border, hover_bg) = (alpha(p.primary, 0.5), alpha(p.primary, 0.03));
                el.cursor_pointer()
                    .hover(move |s| s.border_color(hover_border).bg(hover_bg))
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(|this, _, window, cx| this.add_custom(window, cx)))
            })
            .child(
                div()
                    .flex_none()
                    .size(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(14.0))
                    .bg(violet_bg)
                    .text_color(violet)
                    .child(icon("plus").size(px(20.0))),
            )
            .child(
                div().flex_1().min_w_0().child(div().font_weight(FontWeight::EXTRA_BOLD).child("Add your own")).child(
                    div().text_xs().text_color(p.muted_foreground).child(if full {
                        format!("That's {MAX_CUSTOM}, the most an instance keeps.")
                    } else {
                        "A classifier you run, or any https address that answers the same questions as Jev and Clef. \
                         What it gets and answers is in docs/automod.md."
                            .to_owned()
                    }),
                ),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(intro)
            .child(div().flex().justify_end().child(self.reset_badge("automod_providers", &providers_default, p, cx)))
            .child(cards)
            .child(add)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn provider_card(
        &mut self,
        n: usize,
        slot: u64,
        provider: &pb::AutoModProviderSettings,
        saved: Option<&pb::AutoModProviderSettings>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let custom = admin::is_custom(provider);
        let known = admin::known(&provider.id);
        let host = if custom { admin::host_of(&provider.url) } else { known.map(|k| k.host.to_owned()) };
        let name = match (custom, known) {
            (true, _) if !provider.name.trim().is_empty() => provider.name.trim().to_owned(),
            (true, _) => "Your provider".to_owned(),
            (false, Some(k)) => k.name.to_owned(),
            (false, None) => provider.id.clone(),
        };
        let shown_host = host.clone().unwrap_or_else(|| "an address you pick".to_owned());
        let blurb = match known {
            _ if custom => "Yours, at an https address you pick. It gets the same questions as Jev and Clef.",
            Some(k) => k.blurb,
            None => "",
        };
        let hue = known.map_or(0.8, |k| k.hue);
        let tint = hsla(hue, 0.75, if p.dark { 0.68 } else { 0.48 }, 1.0);
        let missing = admin::missing_for(provider, saved);
        let enabled = provider.enabled;
        let live = saved.is_some_and(|s| s.enabled);
        let moved = admin::moved(provider, saved);
        let clef = provider.id == "cloudflare-clef";
        let Some(fields) = self.fields.get(n) else { return div().into_any_element() };
        let (key, account, name_box, url_box, header_box, model_box) = (
            fields.key.clone(),
            fields.account.clone(),
            fields.name.clone(),
            fields.url.clone(),
            fields.header.clone(),
            fields.model.clone(),
        );

        let badge = motion::once(
            div()
                .flex_none()
                .size(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(corner(14.0))
                .bg(tint.opacity(0.18))
                .text_color(tint)
                .text_sm()
                .font_weight(FontWeight::EXTRA_BOLD)
                .map(|el| match known {
                    Some(k) if !custom => {
                        el.child(k.name.split(' ').filter_map(|w| w.chars().next()).collect::<String>())
                    }
                    _ => el.child(icon("webhook").size(px(20.0))),
                }),
            SharedString::from(format!("instance-badge-{slot}-{enabled}")),
            Duration::from_millis(400),
            move |el, t| {
                if enabled { el.relative().top(px(-4.0 * (t * std::f32::consts::PI).sin())) } else { el }
            },
        );
        let green = hsla(0.42, 0.7, if p.dark { 0.55 } else { 0.38 }, 1.0);
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .pl(px(16.0))
            .child(badge)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.muted_foreground)
                                    .when(host.is_none(), |el| el.italic())
                                    .child(shown_host.clone()),
                            )
                            .when(live, |el| {
                                el.child(motion::rise(
                                    pill("SERVERS CAN USE IT", green),
                                    SharedString::from(format!("instance-live-{slot}")),
                                    Duration::ZERO,
                                    4.0,
                                ))
                            }),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).child(blurb)),
            )
            .child(switch(
                SharedString::from(format!("instance-provider-on-{slot}")),
                enabled,
                !enabled && missing.is_some(),
                cx,
                move |this, on, cx| {
                    let Some(n) = this.fields.iter().position(|f| f.slot == slot) else { return };
                    this.patch(cx, |d| {
                        if let Some(p) = d.automod_providers.get_mut(n) {
                            p.enabled = on
                        }
                    })
                },
            ));

        let mut body = div().flex().flex_col().gap(px(14.0)).p(px(16.0)).border_t_1().border_color(p.border).child(
            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(corner(14.0))
                .bg(alpha(p.muted, 0.6))
                .text_xs()
                .text_color(p.muted_foreground)
                .child(icon("globe-lock").size(px(16.0)).text_color(p.primary))
                .child(div().flex_1().min_w_0().child(emphasized(
                    &[
                        ("Turned on, messages that servers choose to check go from this instance to ", false),
                        (&shown_host, true),
                        (", which sees their text. Nothing else goes: no names, ids, servers or addresses.", false),
                    ],
                    p,
                ))),
        );
        if custom {
            let bad_url = !provider.url.trim().is_empty() && host.is_none();
            body = body
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(div().flex_1().child(field("Name", Input::new(&name_box))))
                        .child(div().flex_none().w(px(380.0)).child(field("Address", Input::new(&url_box)))),
                )
                .when(bad_url, |el| {
                    el.child(motion::rise(
                        div()
                            .text_xs()
                            .text_color(amber(p))
                            .child("fuwa only talks to https addresses, like https://moderation.example.com/v1/check."),
                        SharedString::from(format!("instance-bad-url-{slot}")),
                        Duration::ZERO,
                        -4.0,
                    ))
                })
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(div().flex_1().child(field("Key header", Input::new(&header_box))))
                        .child(div().flex_1().child(field("Model", Input::new(&model_box)))),
                );
        }
        let key_note = if moved && provider.api_key_set {
            "New address: type the key again.".to_owned()
        } else if provider.api_key_set && provider.api_key.is_empty() {
            format!(
                "Saved, ends in {}. Type to replace it.",
                if provider.api_key_hint.is_empty() { "••••" } else { provider.api_key_hint.as_str() }
            )
        } else if custom {
            "If it needs one. It goes as Authorization: Bearer, or in the header you name.".to_owned()
        } else {
            known.map_or("", |k| k.key_help).to_owned()
        };
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(if clef {
                    "API token"
                } else {
                    "API key"
                }))
                .child(div().text_xs().text_color(p.muted_foreground).child(key_note))
                .child(Input::new(&key).prefix(icon("key-round").size(px(15.0)).text_color(p.muted_foreground)))
                .child(div().text_size(px(11.0)).text_color(p.muted_foreground).child(
                    "Kept on the instance, sealed with its files when it encrypts them, and never shown again.",
                )),
        );
        if clef {
            body = body.child(field("Account id", Input::new(&account)));
        }
        if let Some(k) = known.filter(|k| !custom && k.models.len() > 1) {
            let picked = if provider.model.is_empty() { k.models[0].0 } else { provider.model.as_str() };
            let mut models = div().flex().gap(px(6.0));
            for (i, (id, label, hint)) in k.models.iter().enumerate() {
                let on = *id == picked;
                let value = if i == 0 { String::new() } else { (*id).to_owned() };
                models = models.child(
                    chip(SharedString::from(format!("instance-model-{slot}-{i}")), &format!("{label} · {hint}"), on, p)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(n) = this.fields.iter().position(|f| f.slot == slot) else { return };
                            let value = value.clone();
                            this.patch(cx, |d| {
                                if let Some(p) = d.automod_providers.get_mut(n) {
                                    p.model = value
                                }
                            });
                            this.tried.remove(&slot);
                        })),
                );
            }
            body = body.child(field("Model", models));
        }
        body = body.child(self.tester(slot, missing.as_deref(), p, window, cx));
        if custom {
            body = body.child(
                div()
                    .id(SharedString::from(format!("instance-remove-{slot}")))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .h(px(30.0))
                    .rounded(corner(10.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.destructive)
                    .cursor_pointer()
                    .hover({
                        let bg = alpha(p.destructive, 0.1);
                        move |s| s.bg(bg)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(slot, cx)))
                    .child(icon("trash").size(px(15.0)))
                    .child(format!(
                        "Remove {}",
                        if provider.name.trim().is_empty() { "this provider" } else { provider.name.trim() }
                    )),
            );
        }
        div()
            .rounded(corner(22.0))
            .border_1()
            .overflow_hidden()
            .map(|el| {
                if enabled {
                    el.border_color(alpha(p.primary, 0.4)).bg(alpha(p.primary, 0.03))
                } else if custom {
                    el.border_dashed().border_color(p.border)
                } else {
                    el.border_color(p.border)
                }
            })
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// "Test connection": a sample scam sent through the provider, and what it said.
    fn tester(
        &self,
        slot: u64,
        missing: Option<&str>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let asking = matches!(self.tried.get(&slot), Some(Tried::Asking));
        let ready = missing.is_none();
        let mut button = soft_button(
            SharedString::from(format!("instance-try-{slot}")),
            if asking { "Asking…" } else { "Try a sample scam" },
            p,
        );
        if ready && !asking {
            button = button.on_click(cx.listener(move |this, _, window, cx| this.try_provider(slot, window, cx)));
        } else {
            button = button.opacity(0.5);
        }
        let mut out =
            div().flex().flex_col().gap(px(8.0)).p(px(12.0)).rounded(corner(14.0)).bg(alpha(p.muted, 0.5)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon("flask-conical").size(px(15.0)).text_color(p.primary))
                    .child(div().flex_1().text_sm().font_weight(FontWeight::EXTRA_BOLD).child("Test connection"))
                    .when(asking, |el| el.child(spinner(format!("instance-try-spin-{slot}"), 14.0, window)))
                    .child(button),
            );
        match self.tried.get(&slot) {
            Some(Tried::Answered(answer)) if answer.ok => {
                let green = hsla(0.42, 0.7, if p.dark { 0.55 } else { 0.38 }, 1.0);
                let mut answered = div().flex().flex_col().gap(px(6.0)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(green)
                        .child(icon("check").size(px(13.0)))
                        .child(format!("It answered in {} ms", answer.elapsed_ms)),
                );
                for (n, score) in answer.scores.iter().take(4).enumerate() {
                    let probability = score.probability.clamp(0.0, 1.0);
                    let fill: Hsla = if probability >= 0.8 { p.destructive.into() } else { alpha(p.primary, 0.6) };
                    answered = answered.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_xs()
                            .child(
                                div()
                                    .w(px(96.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::BOLD)
                                    .child(admin::label_name(&score.label)),
                            )
                            .child(div().flex_1().h(px(8.0)).rounded_full().bg(p.background).overflow_hidden().child(
                                motion::once(
                                    div().h_full().rounded_full().bg(fill),
                                    SharedString::from(format!("instance-score-{slot}-{n}-{}", score.label)),
                                    Duration::from_millis(420 + 60 * n as u64),
                                    move |el, t| {
                                        let eased = 1.0 - (1.0 - t).powi(3);
                                        el.w(gpui_kit::relative(probability * eased))
                                    },
                                ),
                            ))
                            .child(
                                div()
                                    .w(px(36.0))
                                    .flex_none()
                                    .text_right()
                                    .child(format!("{}%", (probability * 100.0).round() as i32)),
                            ),
                    );
                }
                out = out.child(motion::rise(
                    answered,
                    SharedString::from(format!("instance-tried-ok-{slot}")),
                    Duration::ZERO,
                    6.0,
                ));
            }
            Some(Tried::Answered(answer)) => out = out.child(failed(&answer.error, slot, p)),
            Some(Tried::Failed(error)) => out = out.child(failed(error, slot, p)),
            _ => {}
        }
        if let Some(missing) = missing {
            out = out.child(div().text_xs().text_color(p.muted_foreground).child(missing.to_owned()));
        }
        out.into_any_element()
    }
}

/// Why a provider didn't answer, as the instance put it.
fn failed(error: &str, slot: u64, p: &Palette) -> AnyElement {
    let mut text = error.to_owned();
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    motion::rise(
        div()
            .flex()
            .items_start()
            .gap(px(6.0))
            .text_xs()
            .text_color(amber(p))
            .child(icon("triangle-alert").size(px(13.0)).mt(px(1.0)))
            .child(div().flex_1().min_w_0().child(text)),
        SharedString::from(format!("instance-tried-failed-{slot}")),
        Duration::ZERO,
        6.0,
    )
    .into_any_element()
}

/// Words with some of them in bold, wrapping as one sentence.
pub(crate) fn emphasized(parts: &[(&str, bool)], p: &Palette) -> gpui_kit::StyledText {
    let bold = gpui_kit::HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        color: Some(p.foreground.into()),
        ..Default::default()
    };
    let mut text = String::new();
    let mut ranges = Vec::new();
    for (part, strong) in parts {
        if *strong {
            ranges.push((text.len()..text.len() + part.len(), bold));
        }
        text.push_str(part);
    }
    gpui_kit::StyledText::new(text).with_highlights(ranges)
}

fn field(label: &str, control: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(label.to_owned()))
        .child(control)
        .into_any_element()
}

impl Render for InstanceSettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let (name, admin) =
            self.core.shared.read(|s| s.instance(&self.key).map(|i| (i.name(), i.admin)).unwrap_or_default());
        if !admin {
            // Signed out, or no longer an admin here.
            cx.defer_in(window, |_, _, cx| cx.emit(InstanceSettingsEvent::Close));
            return div().into_any_element();
        }
        let page = self.page;

        let mut menu = div().flex().flex_col().w(px(220.0)).child(
            div()
                .flex()
                .flex_col()
                .px(px(10.0))
                .pb(px(14.0))
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(name.clone()),
                )
                .child(div().text_xs().text_color(p.muted_foreground).child("Instance settings")),
        );
        menu = menu.child(
            div()
                .h(px(30.0))
                .px(px(10.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child("INSTANCE"),
        );
        let top = 62.0 + 30.0;
        let mut at_y = top;
        for (n, pg) in PAGES.into_iter().enumerate() {
            let on = pg == page;
            if on {
                at_y = top + 40.0 * n as f32;
            }
            let hover = alpha(p.primary, 0.08);
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("imenu-{}", pg.label())))
                    .h(px(38.0))
                    .mb(px(2.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(corner(10.0))
                    .cursor_pointer()
                    .text_color(if on { p.foreground } else { p.muted_foreground })
                    .when(on, |el| el.font_weight(FontWeight::BOLD))
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.open(pg, cx)))
                    .child(icon(pg.glyph()).size(px(17.0)).text_color(if on { p.primary } else { p.muted_foreground }))
                    .child(pg.label()),
            );
        }
        let at = motion::follow("instance-settings-hl", at_y, window, cx);
        let menu = div()
            .relative()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(at))
                    .h(px(38.0))
                    .rounded(corner(10.0))
                    .bg(alpha(p.primary, 0.16)),
            )
            .child(menu);

        self.bar = None;
        let body = if let Some(error) = &self.load_error {
            div().text_sm().text_color(p.muted_foreground).child(error.clone()).into_any_element()
        } else if self.draft.is_none() {
            shimmer_rows(3, &p).into_any_element()
        } else {
            match page {
                Page::Privacy => self.privacy_page(&p, cx),
                Page::Moderation => self.moderation_page(&p, window, cx),
            }
        };
        let changed = self.changed();
        if !changed.is_empty() {
            self.bar = Some(save_bar(
                "instance-save",
                changed.len(),
                self.saving,
                &p,
                cx,
                |this: &mut Self, window, cx| this.discard(window, cx),
                |this: &mut Self, window, cx| this.commit(this.changed(), Vec::new(), window, cx),
            ));
        }
        let content = div()
            .w(px(720.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(page.label()))
            .child(div().text_color(p.muted_foreground).child(page.about()))
            .child(div().h(px(18.0)))
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(div().mb(px(12.0)).child(e)))
            .child(body)
            .child(div().h(px(if self.bar.is_some() { 90.0 } else { 0.0 })));

        motion::fade_in(
            div()
                .id("instance-settings")
                .size_full()
                .occlude()
                .flex()
                .bg(p.background)
                .child(
                    div()
                        .flex_none()
                        .w(px(300.0))
                        .h_full()
                        .flex()
                        .justify_end()
                        .pt(px(56.0))
                        .pr(px(16.0))
                        .bg(p.sidebar)
                        .child(motion::slide_in(menu, "instance-settings-menu", -24.0)),
                )
                .child(
                    div()
                        .id("instance-settings-body")
                        .flex_1()
                        .h_full()
                        .overflow_y_scroll()
                        .pt(px(56.0))
                        .px(px(40.0))
                        .pb(px(40.0))
                        .child(motion::rise(
                            content,
                            SharedString::from(format!("ipage-{}", page.label())),
                            Duration::ZERO,
                            14.0,
                        )),
                )
                .when_some(self.bar.take(), |el, bar| {
                    el.child(
                        div().absolute().bottom(px(24.0)).left(px(300.0)).right_0().flex().justify_center().child(bar),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .top(px(20.0))
                        .right(px(24.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id("instance-settings-close")
                                .size(px(38.0))
                                .rounded_full()
                                .border_2()
                                .border_color(p.muted_foreground)
                                .text_color(p.muted_foreground)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover({
                                    let c = p.primary;
                                    move |s| s.border_color(c).text_color(c)
                                })
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(InstanceSettingsEvent::Close)))
                                .child(icon("x").size(px(18.0))),
                        )
                        .child(
                            div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"),
                        ),
                ),
            "instance-settings-in",
            Duration::from_millis(160),
        )
        .into_any_element()
    }
}
