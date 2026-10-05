//! An instance's settings, for its admins, full screen like a server's:
//! its name and address, sign-ups, caps, the anonymous usage signal, calls
//! and the moderation services servers' AutoMod can ask, as in the web app's
//! `InstanceSettingsDialog.tsx`. Each setting
//! starts from the operator's environment; what's changed here is stored on
//! the instance, and "Back to the default" puts it back.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

mod accounts;
mod announcement;
mod calls;
mod controls;
mod federation;
mod general;
mod limits;
mod servers;
mod signups;
mod sso;

use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    hsla, px,
};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin, MAX_CUSTOM, Missing};
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::{amber, chip, marked, pill, save_bar, shimmer_rows, spinner, strong, switch};

/// What stands in for an address in streamer mode, as on the web.
pub(super) fn hidden_address() -> String {
    t("desktop.instance.addressHidden")
}
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{error_line, icon, pal, soft_button};

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
    Limits,
    Privacy,
    Calls,
    Moderation,
    Federation,
    Accounts,
    Servers,
    Announcement,
}

impl Page {
    fn label(self) -> String {
        match self {
            Page::General => t("instancesettings.nav.general"),
            Page::SignUps => t("instancesettings.nav.signUps"),
            Page::Sso => t("serversettings.nav.sso"),
            Page::Limits => t("serversettings.nav.limits"),
            Page::Privacy => t("instancesettings.nav.privacy"),
            Page::Calls => t("instancesettings.nav.calls"),
            Page::Moderation => t("instancesettings.nav.moderation"),
            Page::Federation => t("instancesettings.nav.federation"),
            Page::Accounts => t("instancesettings.nav.accounts"),
            Page::Servers => t("instancesettings.nav.servers"),
            Page::Announcement => t("instancesettings.nav.announcement"),
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Page::General => "sliders-horizontal",
            Page::SignUps => "user-plus",
            Page::Sso => "building",
            Page::Limits => "gauge",
            Page::Privacy => "shield-check",
            Page::Calls => "audio-lines",
            Page::Moderation => "shield-alert",
            Page::Federation => "network",
            Page::Accounts => "users",
            Page::Servers => "server",
            Page::Announcement => "megaphone",
        }
    }

    fn about(self) -> String {
        match self {
            Page::General => t("desktop.instance.generalAbout"),
            Page::SignUps => t("instancesettings.nav.signUpsAbout"),
            Page::Sso => t("instancesettings.nav.ssoAbout"),
            Page::Limits => t("instancesettings.nav.limitsAbout"),
            Page::Privacy => t("instancesettings.nav.privacyAbout"),
            Page::Calls => t("instancesettings.nav.callsAbout"),
            Page::Moderation => t("instancesettings.nav.moderationAbout"),
            Page::Federation => t("instancesettings.nav.federationAbout"),
            Page::Accounts => t("instancesettings.nav.accountsAbout"),
            Page::Servers => t("desktop.instance.serversAbout"),
            Page::Announcement => t("instancesettings.nav.announcementAbout"),
        }
    }

    /// Pages that look after the instance rather than change its settings.
    fn manages(self) -> bool {
        matches!(self, Page::Accounts | Page::Servers | Page::Announcement)
    }
}

/// The menu's groups (instance, manage) and their pages, in the web's order.
const GROUPS: [(bool, &[Page]); 2] = [
    (
        false,
        &[
            Page::General,
            Page::SignUps,
            Page::Sso,
            Page::Limits,
            Page::Privacy,
            Page::Calls,
            Page::Moderation,
            Page::Federation,
        ],
    ),
    (true, &[Page::Accounts, Page::Servers, Page::Announcement]),
];

type GetText = fn(&pb::InstanceSettings) -> String;
type SetText = fn(&mut pb::InstanceSettings, String);

/// Settings typed in one line: (path, placeholder, read, write). The TURN secret's placeholder
/// says whether one is saved, so it's set as the draft comes in.
const TEXTS: [(&str, &str, GetText, SetText); 4] = [
    ("name", "", |s| s.name.clone(), |s, v| s.name = v.chars().take(64).collect()),
    ("public_url", "https://chat.example.com", |s| s.public_url.clone(), |s, v| s.public_url = v),
    ("linked_issuer", signups::WAIFU_DEV_ISSUER, |s| s.linked_issuer.clone(), |s, v| s.linked_issuer = v),
    ("turn_secret", "", |s| s.turn_secret.clone(), |s, v| s.turn_secret = v),
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
fn signal() -> [String; 5] {
    [
        t("instancesettings.privacy.counts"),
        t("instancesettings.privacy.storage"),
        t("instancesettings.privacy.options"),
        t("instancesettings.privacy.version"),
        t("instancesettings.privacy.errors"),
    ]
}

/// What a known provider is, in a line.
fn blurb(id: &str) -> String {
    match id {
        "typesafe-jev" => t("instancesettings.moderation.jevBlurb"),
        "cloudflare-clef" => t("instancesettings.moderation.clefBlurb"),
        _ => String::new(),
    }
}

/// Where a known provider's key comes from.
fn key_help(id: &str) -> String {
    match id {
        "typesafe-jev" => t("instancesettings.moderation.jevKeyHelp"),
        "cloudflare-clef" => t("instancesettings.moderation.clefKeyHelp"),
        _ => String::new(),
    }
}

/// What a known provider's model is good for.
fn model_hint(id: &str) -> String {
    match id {
        "jev-latest" => t("instancesettings.moderation.jevLatest"),
        "jev-preview" => t("instancesettings.moderation.jevPreview"),
        "@cf/cloudflare/clef" => t("instancesettings.moderation.clefModel"),
        "@cf/cloudflare/clef-flash" => t("instancesettings.moderation.clefFlash"),
        _ => String::new(),
    }
}

/// What a provider's card still needs, as a sentence.
fn missing_text(missing: Missing) -> String {
    match missing {
        Missing::Key | Missing::Custom { name: false, address: false, key: true } => {
            t("instancesettings.moderation.addKey")
        }
        Missing::TokenAndAccount => t("instancesettings.moderation.addTokenAccount"),
        Missing::Custom { name: true, address: false, key: false } => t("instancesettings.moderation.addName"),
        Missing::Custom { name: false, address: true, key: false } => t("instancesettings.moderation.addAddress"),
        Missing::Custom { name: true, address: true, key: false } => t("instancesettings.moderation.addNameAddress"),
        Missing::Custom { name: true, address: false, key: true } => t("instancesettings.moderation.addNameKey"),
        Missing::Custom { name: false, address: true, key: true } => t("instancesettings.moderation.addAddressKey"),
        Missing::Custom { name: true, address: true, key: true } => t("instancesettings.moderation.addAll"),
        Missing::Custom { name: false, address: false, key: false } => String::new(),
    }
}

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
    /// The boxes settings are typed in, by path; they last as long as the view.
    texts: HashMap<&'static str, Entity<InputState>>,
    areas: HashMap<&'static str, Entity<TextareaState>>,
    caps: HashMap<&'static str, Entity<InputState>>,
    /// The unit each size cap is typed in.
    units: HashMap<&'static str, usize>,
    bar: Option<AnyElement>,
    announce: announcement::Announce,
    sso: sso::Sso,
    accounts: accounts::Accounts,
    servers: servers::Servers,
    federation: federation::Federation,
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
        let mut view = Self {
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
            bar: None,
            announce,
            sso,
            accounts,
            servers,
            federation,
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
            // The header's name, as it goes over the wire.
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
            let placeholder =
                if path == "turn_secret" { t("instancesettings.calls.noSecret") } else { placeholder.to_owned() };
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

    /// Esc closes a dialog over the page first; false when there was none.
    pub fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        match self.page {
            Page::Servers => self.escape_servers(cx),
            _ => self.close_account_dialog(cx),
        }
    }

    fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Servers && self.page == Page::Servers {
            self.servers_back(cx);
        }
        self.page = page;
        self.error = None;
        cx.notify();
    }

    fn privacy_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(defaults)) =
            (self.draft.as_ref(), self.config.as_ref().and_then(|c| c.defaults.as_ref()))
        else {
            return div().into_any_element();
        };
        let on = draft.telemetry;
        let default = signups::on_off(defaults.telemetry);
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (n, line) in signal().into_iter().enumerate() {
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(icon("shield-check").size(px(14.0)).text_color(p.primary))
                    .child(line),
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
                        div().flex_1().font_weight(FontWeight::EXTRA_BOLD).child(t("instancesettings.nav.telemetry")),
                    )
                    .child(self.reset_badge(&["telemetry"], &default, p, cx)),
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
                                    .child(t("instancesettings.privacy.label")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(t("instancesettings.privacy.hint")),
                            ),
                    )
                    .child(switch("instance-telemetry-switch".into(), on, false, cx, |this, on, cx| {
                        this.patch(cx, |d| d.telemetry = on)
                    })),
            )
            .child(list)
            .child(div().text_xs().text_color(p.muted_foreground).child(t("desktop.instance.signalFields")))
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
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.moderation.intro")),
                ),
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
        let providers_default = match on.split_first() {
            None => t("desktop.instance.providersAllOff"),
            Some((first, rest)) => {
                let names = rest.iter().fold(first.clone(), |names, next| {
                    t_with("desktop.instance.listAnd", &[("first", Arg::Str(&names)), ("second", Arg::Str(next))])
                });
                t_with("desktop.instance.providersOn", &[("names", Arg::Str(&names))])
            }
        };
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
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("instancesettings.moderation.addOwn")))
                    .child(div().text_xs().text_color(p.muted_foreground).child(if full {
                        t_with("instancesettings.moderation.max", &[("count", Arg::Num(MAX_CUSTOM as i64))])
                    } else {
                        t("instancesettings.moderation.addOwnHint")
                    })),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(intro)
            .child(self.daily_checks(p, window, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().font_weight(FontWeight::EXTRA_BOLD).child(t("desktop.instance.providers")))
                    .child(self.reset_badge(&["automod_providers"], &providers_default, p, cx)),
            )
            .child(cards)
            .child(add)
            .into_any_element()
    }

    /// How many times a day each server's smart filter may ask, as on the web.
    fn daily_checks(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let default =
            self.config.as_ref().and_then(|c| c.defaults.as_ref()).and_then(|d| d.automod_checks_per_day).map_or_else(
                || t("instancesettings.shared.noLimit"),
                |n| t_with("instancesettings.shared.perDay", &[("count", Arg::Num(n))]),
            );
        let hint = marked(
            &t_with(
                "desktop.instance.checksHint",
                &[("unchecked", Arg::Str(&strong(&t("desktop.instance.checksUnchecked"))))],
            ),
            p,
        );
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().text_sm().text_color(p.muted_foreground).child(hint))
            .child(self.cap("automod_checks_per_day", &t("instancesettings.shared.upTo"), false, p, window, cx));
        self.setting(
            "automod-checks-per-day",
            &t("instancesettings.nav.automodChecks"),
            None,
            &["automod_checks_per_day"],
            &default,
            1,
            body,
            p,
            cx,
        )
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
            (true, _) => t("instancesettings.moderation.yourProvider"),
            (false, Some(k)) => k.name.to_owned(),
            (false, None) => provider.id.clone(),
        };
        let shown_host = host.clone().unwrap_or_else(|| t("instancesettings.moderation.anAddress"));
        let blurb = if custom { t("instancesettings.moderation.customBlurb") } else { blurb(&provider.id) };
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
                                    pill(&t("instancesettings.moderation.live").to_uppercase(), green),
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
                .child(div().flex_1().min_w_0().child(marked(
                    &t_with("instancesettings.moderation.goesTo", &[("host", Arg::Str(&strong(&shown_host)))]),
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
                        .child(div().flex_1().child(field(&t("instancesettings.nav.name"), Input::new(&name_box))))
                        .child(
                            div()
                                .flex_none()
                                .w(px(380.0))
                                .child(field(&t("instancesettings.moderation.address"), Input::new(&url_box))),
                        ),
                )
                .when(bad_url, |el| {
                    el.child(motion::rise(
                        div().text_xs().text_color(amber(p)).child(t("instancesettings.moderation.httpsOnly")),
                        SharedString::from(format!("instance-bad-url-{slot}")),
                        Duration::ZERO,
                        -4.0,
                    ))
                })
                .child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .child(field(&t("instancesettings.moderation.keyHeader"), Input::new(&header_box))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .child(field(&t("instancesettings.moderation.model"), Input::new(&model_box))),
                        ),
                );
        }
        let key_note = if moved && provider.api_key_set {
            t("desktop.instance.retypeKey")
        } else if provider.api_key_set && provider.api_key.is_empty() {
            let hint = if provider.api_key_hint.is_empty() { "••••" } else { provider.api_key_hint.as_str() };
            t_with("instancesettings.moderation.savedEnds", &[("hint", Arg::Str(hint))])
        } else if custom {
            t("instancesettings.moderation.customKeyHelp")
        } else {
            key_help(&provider.id)
        };
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(if clef {
                    t("instancesettings.moderation.apiToken")
                } else {
                    t("instancesettings.moderation.apiKey")
                }))
                .child(div().text_xs().text_color(p.muted_foreground).child(key_note))
                .child(Input::new(&key).prefix(icon("key-round").size(px(15.0)).text_color(p.muted_foreground)))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.moderation.kept")),
                ),
        );
        if clef {
            body = body.child(field(&t("instancesettings.moderation.accountId"), Input::new(&account)));
        }
        if let Some(k) = known.filter(|k| !custom && k.models.len() > 1) {
            let picked = if provider.model.is_empty() { k.models[0].0 } else { provider.model.as_str() };
            let mut models = div().flex().gap(px(6.0));
            for (i, (id, label)) in k.models.iter().enumerate() {
                let on = *id == picked;
                let value = if i == 0 { String::new() } else { (*id).to_owned() };
                models = models.child(
                    chip(
                        SharedString::from(format!("instance-model-{slot}-{i}")),
                        &format!("{label} · {}", model_hint(id)),
                        on,
                        p,
                    )
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
            body = body.child(field(&t("instancesettings.moderation.model"), models));
        }
        body = body.child(self.tester(slot, missing.map(missing_text), p, window, cx));
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
                    .child(if provider.name.trim().is_empty() {
                        t("instancesettings.moderation.removeThis")
                    } else {
                        t_with("instancesettings.moderation.remove", &[("name", Arg::Str(provider.name.trim()))])
                    }),
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
        missing: Option<String>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let asking = matches!(self.tried.get(&slot), Some(Tried::Asking));
        let ready = missing.is_none();
        let mut button = soft_button(
            SharedString::from(format!("instance-try-{slot}")),
            if asking { t("instancesettings.moderation.asking") } else { t("instancesettings.moderation.trySample") },
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
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("instancesettings.moderation.test")),
                    )
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
                        .child(t_with(
                            "instancesettings.moderation.answered",
                            &[("ms", Arg::Str(&answer.elapsed_ms.to_string()))],
                        )),
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
            out = out.child(div().text_xs().text_color(p.muted_foreground).child(missing));
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
                .child(div().text_xs().text_color(p.muted_foreground).child(t("instancesettings.nav.subtitle"))),
        );
        let mut at_y = 0.0;
        let mut y = 62.0;
        for (manage, pages) in GROUPS {
            let group = if manage { t("instancesettings.nav.manage") } else { t("instancesettings.nav.instance") };
            menu = menu.child(
                div()
                    .h(px(30.0))
                    .px(px(10.0))
                    .flex()
                    .items_end()
                    .pb(px(8.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(group.to_uppercase()),
            );
            y += 30.0;
            for &pg in pages {
                let on = pg == page;
                if on {
                    at_y = y;
                }
                y += 40.0;
                let hover = alpha(p.primary, 0.08);
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("imenu-{pg:?}")))
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
                        .child(icon(pg.glyph()).size(px(17.0)).text_color(if on {
                            p.primary
                        } else {
                            p.muted_foreground
                        }))
                        .child(pg.label()),
                );
            }
            y += 10.0;
            menu = menu.child(div().h(px(10.0)));
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
        let body = if page.manages() {
            match page {
                Page::Accounts => self.accounts_page(&p, window, cx),
                Page::Servers => self.servers_page(&p, window, cx),
                Page::Announcement => self.announcement_page(&p, window, cx),
                _ => div().into_any_element(),
            }
        } else if let Some(error) = &self.load_error {
            div().text_sm().text_color(p.muted_foreground).child(error.clone()).into_any_element()
        } else if self.draft.is_none() {
            shimmer_rows(3, &p).into_any_element()
        } else {
            match page {
                Page::General => self.general_page(&p, window, cx),
                Page::SignUps => self.signups_page(&p, window, cx),
                Page::Sso => self.sso_page(&p, window, cx),
                Page::Limits => self.limits_page(&p, window, cx),
                Page::Calls => self.calls_page(&p, window, cx),
                Page::Privacy => self.privacy_page(&p, cx),
                Page::Moderation => self.moderation_page(&p, window, cx),
                Page::Federation => self.federation_page(&p, window, cx),
                Page::Accounts | Page::Servers | Page::Announcement => div().into_any_element(),
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
        // The accounts and servers lists scroll on their own, drawing only the rows in sight.
        let fills = page == Page::Accounts || (page == Page::Servers && self.servers_fill());
        let content = div()
            .w(px(720.0))
            .when(fills, |el| el.h_full())
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(page.label()))
            .child(div().text_color(p.muted_foreground).child(page.about()))
            .child(div().h(px(18.0)))
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(div().mb(px(12.0)).child(e)))
            .child(body)
            .child(div().h(px(if self.bar.is_some() { 90.0 } else { 0.0 })));

        let dialog = match page {
            Page::Servers => self.share_dialog(&p, cx),
            _ => self.account_dialog(&p, window, cx),
        };
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
                        .when(!fills, |el| el.overflow_y_scroll())
                        .pt(px(56.0))
                        .px(px(40.0))
                        .pb(px(40.0))
                        .child(motion::rise(
                            content,
                            SharedString::from(format!("ipage-{page:?}")),
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
                )
                .when_some(dialog, |el, dialog| el.child(dialog)),
            "instance-settings-in",
            Duration::from_millis(160),
        )
        .into_any_element()
    }
}
