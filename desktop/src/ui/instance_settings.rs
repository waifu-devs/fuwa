//! An instance's settings, for its admins, full screen like a server's:
//! its name and address, sign-ups, caps, the anonymous usage signal, calls
//! and the moderation services servers' AutoMod can ask, as in the web app's
//! `InstanceSettingsDialog.tsx`. Each setting
//! starts from the operator's environment; what's changed here is stored on
//! the instance, and "Back to the default" puts it back.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod accounts;
mod announcement;
mod calls;
mod controls;
mod federation;
mod frame;
mod general;
mod gifs;
mod limits;
mod moderation;
mod providers;
mod release;
mod servers;
mod signups;
mod sso;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin, MAX_CUSTOM};
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::Palette;
use crate::ui::widgets::{error_line, icon, pal};

/// What stands in for an address in streamer mode, as on the web.
const HIDDEN_ADDRESS: &str = "address hidden";

pub enum InstanceSettingsEvent {
    Close,
    /// A short note in the app's corner, like the web's toasts.
    Toast {
        icon: &'static str,
        title: String,
    },
    /// Close settings and go to this server.
    OpenServer(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    General,
    SignUps,
    Sso,
    Providers,
    Limits,
    Privacy,
    Calls,
    Moderation,
    Federation,
    Gifs,
    Accounts,
    Servers,
    ProfileItems,
    Announcement,
}

impl Page {
    /// Pages that look after the instance rather than change its settings.
    fn manages(self) -> bool {
        matches!(self, Page::Accounts | Page::Servers | Page::ProfileItems | Page::Announcement)
    }
}

type GetText = fn(&pb::InstanceSettings) -> String;
type SetText = fn(&mut pb::InstanceSettings, String);

/// Settings typed in one line: (path, placeholder, read, write).
const TEXTS: [(&str, &str, GetText, SetText); 4] = [
    ("name", "", |s| s.name.clone(), |s, v| s.name = v.chars().take(64).collect()),
    ("public_url", "https://chat.example.com", |s| s.public_url.clone(), |s, v| s.public_url = v),
    ("linked_issuer", signups::WAIFU_DEV_ISSUER, |s| s.linked_issuer.clone(), |s, v| s.linked_issuer = v),
    ("turn_secret", "No TURN secret", |s| s.turn_secret.clone(), |s, v| s.turn_secret = v),
];

/// Lists typed one per line: (path, placeholder, read, write).
const AREAS: [(&str, &str, GetText, SetText); 3] = [
    (
        "allowed_origins",
        "https://fuwa.waifu.dev\nhttps://chat.example.com",
        |s| s.allowed_origins.iter().filter(|o| *o != "*").cloned().collect::<Vec<_>>().join("\n"),
        |s, v| s.allowed_origins = v.lines().map(|o| o.trim().to_owned()).filter(|o| !o.is_empty()).collect(),
    ),
    (
        "ice_urls",
        "stun:stun.example.com:3478\nturn:turn.example.com:3478?transport=udp",
        |s| s.ice_urls.join("\n"),
        |s, v| s.ice_urls = v.split('\n').map(str::to_owned).collect(),
    ),
    (
        "federation_blocked_hosts",
        "spam.example.com\nchat.example.org",
        |s| s.federation_blocked_hosts.join("\n"),
        |s, v| s.federation_blocked_hosts = admin::hosts(&v.lines().collect::<Vec<_>>()),
    ),
];

/// A list as the instance reads it: trimmed, blank lines dropped.
fn kept(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// What the usage signal counts, as on the web.
const SIGNAL: [&str; 5] = [
    "instancesettings.privacy.counts",
    "instancesettings.privacy.storage",
    "instancesettings.privacy.options",
    "instancesettings.privacy.version",
    "instancesettings.privacy.errors",
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
    /// The Profile items page, made when first opened.
    profile_items: Option<Entity<crate::ui::profile_items::ProfileItemsView>>,
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
    /// The boxes settings are typed in, by path; they last as long as the view.
    texts: HashMap<&'static str, Entity<InputState>>,
    areas: HashMap<&'static str, Entity<TextareaState>>,
    caps: HashMap<&'static str, Entity<InputState>>,
    /// The unit each size cap is typed in.
    units: HashMap<&'static str, usize>,
    announce: announcement::Announce,
    sso: sso::Sso,
    accounts: accounts::Accounts,
    servers: servers::Servers,
    federation: federation::Federation,
    gifs: gifs::Gifs,
    providers: providers::Providers,
    /// The copy button that just copied, for its check.
    copied: Option<String>,
    /// The menu's search.
    query: Entity<InputState>,
    /// Escape cleared the search; the box empties when it next draws.
    clear_query: bool,
    focused_once: bool,
    /// The setting search picked, glowing where it landed (and a count, so it glows again).
    glow: Option<(&'static str, u32)>,
    /// A setting to scroll to once its page is on screen.
    scroll_to: Option<&'static str>,
    /// Where each search target was drawn, for scrolling to it.
    places: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    /// The width of the page's column, which option cards share.
    column: f32,
    /// Someone tried to leave with unsaved changes: how many times, and when last.
    nudge: (u32, Option<Instant>),
    /// The placeholders set on boxes whose words change with their state.
    placeholders: HashMap<gpui_kit::EntityId, String>,
    /// When the screen opened, for its fade in.
    opened: Instant,
    /// Closing: the screen fades and grows away, then goes.
    closing: Option<Instant>,
    _subscriptions: Vec<Subscription>,
    _boxes: Vec<Subscription>,
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
        let (announce, announce_sub) = announcement::Announce::new(window, cx);
        let (accounts, mut boxes) = accounts::Accounts::new(window, cx);
        boxes.push(announce_sub);
        let (sso, sso_subs) = sso::Sso::new(window, cx);
        boxes.extend(sso_subs);
        let (servers, servers_subs) = servers::Servers::new(window, cx);
        boxes.extend(servers_subs);
        let (federation, federation_subs) = federation::Federation::new(window, cx);
        boxes.extend(federation_subs);
        let (gifs, gifs_subs) = gifs::Gifs::new(window, cx);
        boxes.extend(gifs_subs);
        let (providers, providers_subs) = providers::Providers::new(window, cx);
        boxes.extend(providers_subs);
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.screen.search")));
        boxes.push(cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => this.pick_first(cx),
            _ => {}
        }));
        let mut view = Self {
            profile_items: None,
            core,
            key,
            page: Page::General,
            config: None,
            draft: None,
            load_error: None,
            saving: false,
            error: None,
            fields: Vec::new(),
            next_slot: 0,
            tried: HashMap::new(),
            texts: HashMap::new(),
            areas: HashMap::new(),
            caps: HashMap::new(),
            units: HashMap::new(),
            announce,
            sso,
            accounts,
            servers,
            federation,
            gifs,
            providers,
            copied: None,
            query,
            clear_query: false,
            focused_once: false,
            glow: None,
            scroll_to: None,
            places: Default::default(),
            scroll: ScrollHandle::new(),
            column: 792.0,
            nudge: (0, None),
            placeholders: HashMap::new(),
            opened: Instant::now(),
            closing: None,
            _subscriptions: Vec::new(),
            _boxes: boxes,
        };
        view.make_boxes(window, cx);
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
                    this.sync_boxes(window, cx);
                }
                Err(problem) => this.load_error = Some(problem.message),
            }
            cx.notify();
        });
    }

    fn saved(&self) -> Option<&pb::InstanceSettings> {
        self.config.as_ref().and_then(|c| c.settings.as_ref())
    }

    /// Whether this instance knows a feature, so its settings show only where it does.
    fn instance_has(&self, feature: &str) -> bool {
        self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.has(feature)))
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
                            for path in &pending {
                                admin::copy_field(&mut fresh, draft, path);
                            }
                        }
                        let providers_from_saved = !pending.iter().any(|p| p == "automod_providers");
                        this.draft = Some(fresh);
                        this.config = Some(config);
                        if providers_from_saved {
                            // Saved keys come back as hints, and new ones get their ids.
                            this.refill(window, cx);
                        }
                        this.sync_boxes(window, cx);
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
        self.sync_boxes(window, cx);
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
            key: input(&p.api_key, &t("instancesettings.moderation.paste"), true, window, cx),
            account: input(&p.account_id, &t("instancesettings.moderation.accountIdPlaceholder"), hide, window, cx),
            name: input(&p.name, &t("instancesettings.moderation.namePlaceholder"), false, window, cx),
            url: input(&p.url, "https://moderation.example.com/v1/check", hide, window, cx),
            header: input(&p.header, "Authorization (Bearer)", false, window, cx),
            model: input(&p.model, &t("instancesettings.moderation.modelOptional"), false, window, cx),
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

    /// The boxes for settings typed as text, each writing to the draft as it changes.
    fn make_boxes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hide = self.core.prefs().streamer_mode;
        for (path, placeholder, get, set) in TEXTS {
            let masked = path == "turn_secret" || (hide && path == "public_url");
            let state = cx.new(|cx| {
                let s = InputState::new(window, cx).placeholder(placeholder);
                if masked { s.masked(true) } else { s }
            });
            let sub = cx.subscribe(&state, move |this: &mut Self, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    if this.draft.as_ref().is_some_and(|d| get(d) != value) {
                        this.patch(cx, |d| set(d, value));
                    }
                }
            });
            self.texts.insert(path, state);
            self._boxes.push(sub);
        }
        for (path, placeholder, get, set) in AREAS {
            let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).placeholder(placeholder));
            let sub = cx.subscribe(&state, move |this: &mut Self, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    if this.draft.as_ref().is_some_and(|d| get(d) != value) {
                        this.patch(cx, |d| set(d, value));
                    }
                }
            });
            self.areas.insert(path, state);
            self._boxes.push(sub);
        }
        for (path, bytes) in limits::CAPS {
            let state = cx.new(|cx| InputState::new(window, cx));
            let sub = cx.subscribe(&state, move |this: &mut Self, state, e: &InputEvent, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                let unit = bytes.then(|| this.units.get(path).copied().unwrap_or(1));
                let Some(value) = admin::parse_cap(&state.read(cx).value(), unit) else { return };
                // A cap that's off stays off while its box is filled in.
                if this.draft.as_ref().and_then(|d| admin::cap(d, path)).is_some_and(|now| now != value) {
                    this.patch(cx, |d| admin::set_cap(d, path, Some(value)));
                }
            });
            self.caps.insert(path, state);
            self.units.insert(path, 1);
            self._boxes.push(sub);
        }
    }

    /// Puts the draft into the boxes that don't already say it (after loading, a save or a discard),
    /// leaving what's typed alone where it means the same.
    fn sync_boxes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_sso(window, cx);
        self.sync_providers(window, cx);
        self.sync_gifs(window, cx);
        let Some(draft) = self.draft.clone() else { return };
        for (path, _, get, _) in TEXTS {
            let Some(state) = self.texts.get(path) else { continue };
            let want = get(&draft);
            if state.read(cx).value().as_ref() != want {
                state.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
        // The saved TURN secret never comes back; an empty box keeps it.
        if let Some(state) = self.texts.get("turn_secret") {
            let saved = match (draft.turn_secret_set, draft.turn_secret_hint.as_str()) {
                (false, _) => t("instancesettings.calls.noSecret"),
                (true, "") => t("instancesettings.shared.saved"),
                (true, end) => t_with("instancesettings.shared.savedEnding", &[("hint", Arg::Str(end))]),
            };
            state.update(cx, |s, cx| s.set_placeholder(saved, window, cx));
        }
        for (path, _, get, _) in AREAS {
            let Some(state) = self.areas.get(path) else { continue };
            let want = get(&draft);
            if path == "allowed_origins" && draft.allowed_origins.iter().any(|o| o == "*") {
                continue;
            }
            if kept(&state.read(cx).value()) != kept(&want) {
                state.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
        for (path, bytes) in limits::CAPS {
            let Some(state) = self.caps.get(path).cloned() else { continue };
            let value = admin::cap(&draft, path);
            let unit = bytes.then(|| self.units.get(path).copied().unwrap_or(1));
            if value.is_none() || admin::parse_cap(&state.read(cx).value(), unit) == value {
                continue;
            }
            let (text, unit) =
                if bytes { admin::split_bytes(value) } else { (value.map(|v| v.to_string()).unwrap_or_default(), 1) };
            self.units.insert(path, unit);
            state.update(cx, |s, cx| s.set_value(text, window, cx));
        }
    }

    /// Turns a cap on (with what's typed, or 100, or 1 GB) or off.
    fn switch_cap(&mut self, path: &'static str, bytes: bool, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !on {
            self.patch(cx, |d| admin::set_cap(d, path, None));
            return;
        }
        let Some(state) = self.caps.get(path).cloned() else { return };
        let unit = bytes.then(|| self.units.get(path).copied().unwrap_or(1));
        let value = match admin::parse_cap(&state.read(cx).value(), unit) {
            Some(v) => v,
            None => {
                let (text, v) = if bytes {
                    ("1".to_owned(), admin::UNITS[1].1)
                } else {
                    let start = admin::starting_cap(path);
                    (start.to_string(), start)
                };
                self.units.insert(path, 1);
                state.update(cx, |s, cx| s.set_value(text, window, cx));
                v
            }
        };
        self.patch(cx, |d| admin::set_cap(d, path, Some(value)));
        state.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Reads a size cap's number in another unit.
    fn pick_unit(&mut self, path: &'static str, unit: usize, cx: &mut Context<Self>) {
        self.units.insert(path, unit);
        let typed = self.caps.get(path).map(|s| s.read(cx).value().to_string()).unwrap_or_default();
        match admin::parse_cap(&typed, Some(unit)) {
            Some(value) => self.patch(cx, |d| admin::set_cap(d, path, Some(value))),
            None => cx.notify(),
        }
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

    /// Esc closes a dialog over the page first, then clears a search, then closes the screen
    /// (unless unsaved changes hold it). It always answers for itself.
    pub fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        let handled = match self.page {
            Page::Servers => self.escape_servers(cx),
            Page::Federation => self.close_federation_dialog(cx),
            _ => self.close_account_menu(cx) || self.close_account_dialog(cx),
        };
        if handled {
            return true;
        }
        if !self.query.read(cx).value().is_empty() {
            self.clear_query = true;
            cx.notify();
            return true;
        }
        self.close(cx);
        true
    }

    fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Servers && self.page == Page::Servers {
            self.servers_back(cx);
        }
        self.page = page;
        self.error = None;
        cx.notify();
    }

    /// The anonymous usage signal, and what it sends (the web's `PrivacySettings`).
    fn privacy_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(defaults)) =
            (self.draft.as_ref(), self.config.as_ref().and_then(|c| c.defaults.as_ref()))
        else {
            return div().into_any_element();
        };
        let on = draft.telemetry;
        let default = t(if defaults.telemetry { "instancesettings.shared.on" } else { "instancesettings.shared.off" });
        // Two columns, as the web's `sm:grid-cols-2`.
        let half = (self.column - 6.0) / 2.0;
        let mut list = div().flex().flex_wrap().gap(px(6.0));
        for (n, line) in SIGNAL.iter().enumerate() {
            list = list.child(controls::slide_after(
                div()
                    .w(px(half))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(icon("shield-check").size(px(14.0)).flex_none().text_color(p.primary))
                    .child(div().flex_1().min_w_0().child(t(line))),
                SharedString::from(format!("signal-line-{n}")),
                -8.0,
                Duration::from_millis(100 + 50 * n as u64),
            ));
        }
        let fields = div()
            .id("instance-telemetry-fields")
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.primary)
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(|_, _, cx| {
                crate::ui::text::open_link("https://github.com/waifu-devs/fuwa#the-anonymous-usage-signal", cx)
            })
            .child(t("instancesettings.privacy.fields"));
        let body = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.0))
            .child(div().w_full().child(self.toggle(
                "telemetry",
                on,
                false,
                &t("instancesettings.privacy.label"),
                &t("instancesettings.privacy.hint"),
                p,
                window,
                cx,
                |d, on| d.telemetry = on,
            )))
            .child(list)
            .child(fields);
        self.setting("telemetry", &t("instancesettings.nav.telemetry"), None, &["telemetry"], &default, 0, body, p, cx)
    }
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

impl Render for InstanceSettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let admin = self.core.shared.read(|s| s.instance(&self.key).map(|i| i.admin).unwrap_or_default());
        if !admin {
            // Signed out, or no longer an admin here.
            cx.defer_in(window, |_, _, cx| cx.emit(InstanceSettingsEvent::Close));
            return div().into_any_element();
        }
        // As on the web, the search takes the keys when settings open.
        if !self.focused_once {
            self.focused_once = true;
            self.query.update(cx, |q, cx| q.focus(window, cx));
        }
        if std::mem::take(&mut self.clear_query) {
            self.query.update(cx, |q, cx| q.set_value("", window, cx));
        }
        let page = self.page;

        // The web's layout: the menu takes 15rem and half of what's left past 67rem; the
        // page's column is at most 60rem, less the close button's 4rem and its padding.
        let width = f32::from(window.viewport_size().width);
        let aside = 240.0 + ((width - 1072.0).max(0.0) / 2.0);
        let main = (width - aside).max(320.0);
        let inner = main.min(960.0);
        self.column = inner - 64.0 - 80.0;

        let groups = frame::groups(&self.name(), self.instance_has("profile-items"));
        let menu = self.menu(&groups, &p, window, cx);
        let (label, about) = groups
            .iter()
            .flat_map(|g| g.sections.iter())
            .find(|s| s.page == page)
            .map(|s| (s.label.clone(), s.about.clone()))
            .unwrap_or_default();

        let body = if page.manages() {
            match page {
                Page::Accounts => self.accounts_page(&p, window, cx),
                Page::Servers => self.servers_page(&p, window, cx),
                Page::Announcement => self.announcement_page(&p, window, cx),
                Page::ProfileItems => {
                    let (core, key) = (self.core.clone(), self.key.clone());
                    self.profile_items
                        .get_or_insert_with(|| {
                            cx.new(|cx| {
                                crate::ui::profile_items::ProfileItemsView::new(
                                    core,
                                    key,
                                    crate::core::profile_items::Scope::Instance,
                                    window,
                                    cx,
                                )
                            })
                        })
                        .clone()
                        .into_any_element()
                }
                _ => div().into_any_element(),
            }
        } else if let Some(error) = &self.load_error {
            let mut text = error.clone();
            if let Some(first) = text.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            div().text_sm().text_color(p.muted_foreground).child(text).into_any_element()
        } else if self.draft.is_none() {
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children((0..3).map(|n| controls::shimmer(n, 112.0, &p)))
                .into_any_element()
        } else {
            match page {
                Page::General => self.general_page(&p, window, cx),
                Page::SignUps => self.signups_page(&p, window, cx),
                Page::Sso => self.sso_page(&p, window, cx),
                Page::Providers => self.providers_page(&p, window, cx),
                Page::Limits => self.limits_page(&p, window, cx),
                Page::Calls => self.calls_page(&p, window, cx),
                Page::Privacy => self.privacy_page(&p, window, cx),
                Page::Moderation => self.moderation_page(&p, window, cx),
                Page::Federation => self.federation_page(&p, window, cx),
                Page::Gifs => self.gifs_page(&p, window, cx),
                Page::Accounts | Page::Servers | Page::ProfileItems | Page::Announcement => div().into_any_element(),
            }
        };

        // A setting picked from search: once it's been drawn, scroll it to the middle.
        if let Some(id) = self.scroll_to {
            let place = self.places.borrow().get(id).copied();
            match place {
                Some(b) => {
                    let view = self.scroll.bounds();
                    let offset = self.scroll.offset();
                    let content_y = f32::from(b.origin.y - view.origin.y - offset.y);
                    let target = content_y - (f32::from(view.size.height) - f32::from(b.size.height)) / 2.0;
                    let most = f32::from(self.scroll.max_offset().y);
                    self.scroll.set_offset(gpui_kit::point(px(0.0), px(-target.clamp(0.0, most.max(0.0)))));
                    self.scroll_to = None;
                }
                None => window.request_animation_frame(),
            }
        }

        // The save bar sits under every page, held at the foot of the window as it scrolls.
        let changed = self.changed();
        let bar = (!changed.is_empty()).then(|| {
            controls::save_bar(
                "instance-save",
                changed.len(),
                self.saving,
                self.error.as_deref(),
                self.alarm(),
                &p,
                cx,
                |this: &mut Self, window, cx| this.commit(this.changed(), Vec::new(), window, cx),
                |this: &mut Self, window, cx| this.discard(window, cx),
            )
        });
        // The accounts and servers lists scroll on their own, drawing only the rows in sight.
        let fills = page == Page::Accounts;
        let header = div()
            .mb(px(24.0))
            .child(div().text_2xl().line_height(px(32.0)).font_weight(FontWeight::EXTRA_BOLD).child(label))
            .when(!about.is_empty(), |el| {
                el.child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(about))
            });
        // Errors of pages that manage things; the settings' own show in the save bar.
        let error = if page.manages() || bar.is_none() { error_line(self.error.as_deref(), &p) } else { None };
        let content = div()
            .w(px(inner - 64.0))
            .flex_none()
            .when(fills, |el| el.h_full())
            .px(px(40.0))
            .pt(px(64.0))
            // The accounts list runs to the window's foot, as the web's page scrolls under it.
            .pb(px(if fills { 0.0 } else { 16.0 }))
            .flex()
            .flex_col()
            .child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .when(fills, |el| el.flex_1().min_h_0())
                    .child(header)
                    .when_some(error, |el, e| el.child(div().mb(px(12.0)).child(e)))
                    .child(div().flex().flex_col().when(fills, |el| el.flex_1().min_h_0()).child(body)),
                SharedString::from(format!("ipage-{page:?}")),
                Duration::ZERO,
                14.0,
            ))
            .when(bar.is_some(), |el| el.child(div().h(px(92.0))));

        let dialog = match page {
            Page::Servers => self.share_dialog(&p, window, cx),
            Page::Federation => self.federation_dialog(&p, window, cx),
            Page::Accounts => self.account_dialog(&p, window, cx).or_else(|| self.account_menu(&p, cx)),
            _ => None,
        };
        let column = self.column;
        // In the flow and `size_full`, like server settings: an absolute screen inside the cached
        // view came out short of the window's right and bottom edges.
        let screen = div()
            .id("instance-settings")
            .relative()
            .size_full()
            .occlude()
            .flex()
            .bg(p.background)
            .text_color(p.foreground)
            .child(
                div()
                    .id("instance-settings-menu")
                    .flex_none()
                    .w(px(aside))
                    .h_full()
                    .flex()
                    .justify_end()
                    .bg(p.side_surface)
                    .border_r_1()
                    .border_color(p.border)
                    .overflow_y_scroll()
                    .child(menu),
            )
            .child(
                div()
                    .id("instance-settings-body")
                    .flex_1()
                    .h_full()
                    .bg(p.chat_surface)
                    .when(!fills, |el| el.overflow_y_scroll().track_scroll(&self.scroll))
                    .child(content),
            )
            .when_some(bar, |el, bar| {
                el.child(div().absolute().bottom(px(8.0)).left(px(aside + 40.0)).w(px(column)).child(bar))
            })
            .child(self.close_button(aside + inner - 64.0, &p, cx))
            .when_some(dialog, |el, dialog| el.child(dialog));

        // It comes in from a little larger, fading up, and goes the same way (the web's
        // opacity and scale 1.04). Worked out here from the clock, not with `with_animation`:
        // the window caches this view, so it asks for its own frames until it settles.
        let reduce = cx.reduce_motion();
        let (since, out) = match self.closing {
            Some(at) => (at.elapsed(), true),
            None => (self.opened.elapsed(), false),
        };
        let done = reduce || since >= frame::OPENING;
        let t = if done { 1.0 } else { since.as_secs_f32() / frame::OPENING.as_secs_f32() };
        let eased = 1.0 - (1.0 - t).powi(5);
        let k = if out { 1.0 - eased } else { eased };
        if !done {
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        }
        let h = f32::from(window.viewport_size().height);
        let screen = if k >= 1.0 { screen } else { screen.opacity(k).top(px(-h * 0.02 * (1.0 - k))) };
        screen.into_any_element()
    }
}
