//! The app's settings, full screen like the web's `SettingsScreen` and
//! `UserSettings`: a side menu of sections (with a search that finds
//! sections and single settings), the chosen section beside it under its
//! heading, and a close button. App settings belong to this computer and
//! every instance on it; the account group belongs to the instance on
//! screen. The desktop adds its own pages (instances, updates, about).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, point, px,
};

use crate::core::Core;
use crate::core::config::Prefs;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::store::{Connection, user_name};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings_account::AccountForm;
use crate::ui::settings_controls::Sliders;
use crate::ui::settings_look::Look;
use crate::ui::text::{TIGHT, WIDE, tracked};
use crate::ui::theme::{Palette, alpha, corner, radius_2xl, radius_lg, radius_md};
use crate::ui::widgets::{avatar, conn_dot, icon, icon_button, pal, primary_button, soft_button};

pub enum SettingsEvent {
    Close,
    Prefs,
    SignIn { key: String },
    AddInstance,
    Toast { icon: &'static str, title: String },
}

/// Every section, by what the web calls it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Page {
    Appearance,
    Themes,
    Backdrop,
    Accessibility,
    Chat,
    Language,
    Notifications,
    Voice,
    Keybinds,
    Streamer,
    Advanced,
    Instances,
    Updates,
    About,
    Profile,
    ServerProfiles,
    Devices,
    SignIn,
    Security,
    Password,
    Linked,
    ServerNotifications,
    Agents,
    Friends,
    Privacy,
    Session,
}

/// A section of the menu, as the web's `SettingsSection`.
pub(crate) struct Section {
    pub page: Page,
    pub glyph: &'static str,
    pub label: String,
    /// A line under the section's heading.
    pub about: String,
    pub danger: bool,
    /// More words search finds it by (English, beside the words shown).
    pub keywords: &'static str,
    /// Single settings search can jump to, as (id, label, keywords).
    pub settings: Vec<(&'static str, String, &'static str)>,
}

impl Section {
    fn new(page: Page, glyph: &'static str, label: String, about: String, keywords: &'static str) -> Self {
        Self { page, glyph, label, about, danger: false, keywords, settings: Vec::new() }
    }

    fn with(mut self, settings: Vec<(&'static str, String, &'static str)>) -> Self {
        self.settings = settings;
        self
    }
}

/// Sections under a heading; a group without one (signing out) sits apart at the end.
pub(crate) struct Group {
    pub label: Option<String>,
    pub sections: Vec<Section>,
}

fn app_sections() -> Vec<Section> {
    vec![
        Section::new(
            Page::Appearance,
            "palette",
            t("settings.nav.appearance"),
            t("settings.nav.appearanceAbout"),
            "look theme dark light",
        )
        .with(vec![
            ("theme", t("appsettings.appearance.theme"), "dark light system colors"),
            ("density", t("appsettings.appearance.density"), "spacing compact spacious"),
            ("message-display", t("appsettings.appearance.display"), "cozy compact"),
            ("chat-font-size", t("settings.nav.chatTextSize"), "font scaling"),
            ("zoom", t("appsettings.appearance.zoom"), "scale size"),
        ]),
        Section::new(
            Page::Themes,
            "wand-sparkles",
            t("settings.nav.themes"),
            t("settings.nav.themesAbout"),
            "custom theme colors import export file editor",
        ),
        Section::new(
            Page::Backdrop,
            "image",
            t("settings.nav.backdrop"),
            t("settings.nav.backdropAbout"),
            "wallpaper background picture image effect shader aurora petals stars waves grain texture",
        )
        .with(vec![("backdrop", t("appsettings.backgrounds.title"), "wallpaper picture shader texture")]),
        Section::new(
            Page::Accessibility,
            "accessibility",
            t("settings.nav.accessibility"),
            t("settings.nav.accessibilityAbout"),
            "a11y",
        )
        .with(vec![
            ("reduce-motion", t("settings.nav.reduceMotion"), "animation"),
            ("saturation", t("appsettings.accessibility.saturation"), "color grey"),
            ("role-colors", t("appsettings.accessibility.roleColors"), "names colour tint dot"),
            ("others-effects", t("appsettings.accessibility.effects"), "animation sparkles others cards"),
            ("underline-links", t("settings.nav.underlineLinks"), ""),
        ]),
        Section::new(
            Page::Chat,
            "message-square-text",
            t("settings.nav.chat"),
            t("settings.nav.chatAbout"),
            "messages",
        )
        .with(vec![
            ("clock", t("appsettings.chat.clock"), "12 24 hour clock"),
            ("send-with", t("appsettings.chat.sendWith"), "enter newline"),
        ]),
        Section::new(
            Page::Language,
            "languages",
            t("settings.language.title"),
            t("settings.nav.languageAbout"),
            "locale translation idioma español",
        )
        .with(vec![("language", t("settings.language.title"), "locale translation")]),
        Section::new(
            Page::Notifications,
            "bell",
            t("settings.nav.notifications"),
            t("settings.nav.notificationsAbout"),
            "alerts",
        )
        .with(vec![
            ("desktop-notifications", t("appsettings.notifications.desktop"), ""),
            ("notify-for", t("appsettings.notifications.notifyFor"), "mentions every message"),
            ("unread-badge", t("appsettings.notifications.unreadBadge"), "badge title favicon"),
            ("sounds", t("appsettings.notifications.sounds"), "volume audio direct message file custom tune"),
        ]),
        Section::new(
            Page::Voice,
            "audio-lines",
            t("settings.nav.voice"),
            t("settings.nav.voiceAbout"),
            "call microphone mic speakers headset audio camera video webcam",
        )
        .with(crate::ui::settings_voice::voice_settings()),
        Section::new(
            Page::Keybinds,
            "keyboard",
            t("settings.nav.keybinds"),
            t("settings.nav.keybindsAbout"),
            "keyboard shortcuts hotkeys",
        )
        .with(crate::ui::settings_keys::keybind_settings()),
        Section::new(
            Page::Streamer,
            "tv-minimal-play",
            t("appsettings.streamer.title"),
            t("settings.nav.streamerAbout"),
            "privacy obs stream hide",
        )
        .with(vec![
            ("streamer-hide-personal", t("appsettings.streamer.hidePersonal"), "address username"),
            ("streamer-sounds", t("settings.nav.streamerSounds"), ""),
            ("streamer-notifications", t("settings.nav.streamerNotifications"), ""),
        ]),
        Section::new(Page::Advanced, "code-xml", t("settings.nav.advanced"), t("settings.nav.advancedAbout"), "").with(
            vec![
                (
                    "share-reports",
                    t("appsettings.advanced.reports"),
                    "anonymous reports telemetry crash errors performance privacy games",
                ),
                ("developer-mode", t("appsettings.advanced.developer"), "copy id"),
            ],
        ),
    ]
}

/// The desktop app's own sections.
fn desktop_sections() -> Vec<Section> {
    vec![
        Section::new(
            Page::Instances,
            "server",
            t("connect.accounts.title"),
            t("desktop.settings.accountsAbout"),
            "instances accounts add remove sign in",
        ),
        Section::new(
            Page::Updates,
            "refresh-cw",
            t("desktop.settings.updates"),
            t("desktop.settings.updatesAbout"),
            "update version download",
        ),
        Section::new(Page::About, "info", t("desktop.settings.about"), String::new(), "version credits source license"),
    ]
}

fn account_sections(me: &pb::User, place: &str, issuer: &str) -> Vec<Section> {
    let at = |key: &str| t_with(key, &[("instance", Arg::Str(place))]);
    let mut list = vec![
        Section::new(
            Page::Profile,
            "user-round",
            t("settings.nav.profile"),
            at("settings.nav.profileAbout"),
            "name avatar picture",
        )
        .with(vec![
            ("display-name", t("settings.nav.displayName"), ""),
            ("pronouns", t("settings.nav.pronouns"), ""),
            ("avatar", t("settings.nav.avatar"), "picture photo upload image gif"),
            ("banner", t("settings.nav.banner"), "header picture upload image"),
            ("profile-color", t("settings.nav.profileColor"), "accent"),
            (
                "profile-effect",
                t("settings.nav.profileEffect"),
                "sparkles petals stars hearts snow confetti animation decoration",
            ),
            ("status", t("settings.nav.status"), "away busy"),
            ("about-me", t("settings.nav.aboutMe"), "bio description"),
        ]),
        Section::new(
            Page::ServerProfiles,
            "id-card",
            t("settings.nav.serverProfiles"),
            t("settings.nav.serverProfilesAbout"),
            "per server identity",
        )
        .with(vec![("nickname", t("settings.nav.nickname"), "server name")]),
        Section::new(
            Page::Devices,
            "monitor-smartphone",
            t("settings.nav.devices"),
            at("settings.nav.devicesAbout"),
            "sessions sign out log out phone browser",
        ),
        Section::new(
            Page::SignIn,
            "log-in",
            t("settings.nav.signInMethods"),
            at("settings.nav.signInMethodsAbout"),
            "google x twitter twitch link unlink connect sso single sign-on login provider",
        )
        .with(vec![("link-provider", t("settings.nav.linkProvider"), "google x twitch connect")]),
    ];
    if me.kind == pb::AccountKind::Local as i32 {
        list.push(
            Section::new(
                Page::Security,
                "shield-check",
                t("settings.nav.twoStep"),
                t("settings.nav.twoStepAbout"),
                "2fa mfa totp authenticator backup codes security",
            )
            .with(vec![
                ("two-step", t("settings.nav.twoStep"), "2fa authenticator"),
                ("backup-codes", t("settings.nav.backupCodes"), "recovery"),
            ]),
        );
        list.push(Section::new(
            Page::Password,
            "key-round",
            t("settings.nav.password"),
            at("settings.nav.passwordAbout"),
            "security change",
        ));
    } else if me.kind != pb::AccountKind::Provider as i32 {
        list.push(Section::new(
            Page::Linked,
            "flower-2",
            t("settings.nav.linked"),
            t_with("settings.nav.linkedAbout", &[("instance", Arg::Str(place)), ("issuer", Arg::Str(issuer))]),
            "waifu.dev linked password 2fa security",
        ));
    }
    list.extend([
        Section::new(
            Page::ServerNotifications,
            "bell-ring",
            t("settings.nav.serverNotifications"),
            t("settings.nav.serverNotificationsAbout"),
            "mute mentions everyone here alerts",
        ),
        Section::new(
            Page::Agents,
            "bot",
            t("settings.nav.agents"),
            t("settings.nav.agentsAbout"),
            "bot bots token api key automation integration developer",
        ),
        Section::new(
            Page::Friends,
            "heart-handshake",
            t("settings.nav.friends"),
            t("settings.nav.friendsAbout"),
            "friend requests direct messages dm online status mutual block",
        )
        .with(vec![
            ("friend-requests", t("settings.nav.friendRequests"), "requests add"),
            ("direct-messages", t("settings.nav.directMessages"), "dm messages"),
            ("friends-see", t("settings.nav.friendsSee"), "online mutual"),
        ]),
        Section::new(
            Page::Privacy,
            "database",
            t("settings.nav.privacy"),
            at("settings.nav.privacyAbout"),
            "export download delete account gdpr activity rich presence game playing status",
        )
        .with(vec![
            (
                "activity-sharing",
                t("settings.nav.activity"),
                "rich presence activity game playing listening discord share",
            ),
            ("export", t("settings.nav.export"), "export json"),
            ("delete-account", t("settings.nav.deleteAccount"), "remove close"),
        ]),
    ]);
    list
}

/// The issuer's name for a linked account ("waifu.dev", or the issuer's host).
pub(crate) fn issuer_name(issuer: &str) -> String {
    if issuer.is_empty() || issuer == "https://api.waifu.dev" {
        return "waifu.dev".into();
    }
    url::Url::parse(issuer).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| issuer.to_owned())
}

/// A section and the settings in it a search found.
type Found<'a> = (&'a Section, Vec<&'a (&'static str, String, &'static str)>);

/// Sections and single settings whose words hold every word typed (the web's `search`).
fn search<'a>(groups: &'a [Group], query: &str) -> Option<Vec<Found<'a>>> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return None;
    }
    let hits = |hay: &str| {
        let hay = hay.to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    };
    let mut out = Vec::new();
    for section in groups.iter().flat_map(|g| g.sections.iter()) {
        let own = format!("{} {} {}", section.label, section.about, section.keywords);
        let settings: Vec<_> =
            section.settings.iter().filter(|s| hits(&format!("{} {} {}", s.1, s.2, section.label))).collect();
        if !settings.is_empty() || hits(&own) {
            out.push((section, settings));
        }
    }
    Some(out)
}

pub struct SettingsView {
    pub(crate) core: Arc<Core>,
    pub(crate) page: Page,
    pub(crate) keys: crate::ui::settings_keys::Keys,
    pub(crate) account: AccountForm,
    pub(crate) look: Look,
    /// The reports' counts last shown on the Advanced page.
    pub(crate) pending: crate::core::reports::Pending,
    /// Why the last friends setting didn't save.
    pub(crate) friends_error: Option<String>,
    /// The sound whose file is being copied in, and why the last one wasn't taken.
    pub(crate) sound_busy: Option<crate::core::sounds::Sound>,
    pub(crate) sound_error: Option<(crate::core::sounds::Sound, String)>,
    pub(crate) sliders: Sliders,
    /// The menu's search.
    query: Entity<InputState>,
    /// The setting search picked, glowing where it landed (and a count, so it glows again).
    pub(crate) glow: Option<(&'static str, u32)>,
    /// A setting to scroll to once its section is on screen.
    scroll_to: Option<&'static str>,
    /// Where each search target was drawn, for scrolling to it.
    pub(crate) places: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    /// The width the section's column has, and whether a preview fits beside a form.
    pub(crate) column: f32,
    pub(crate) wide: bool,
    /// Unsaved edits on the page on screen (set while it draws); leaving shakes the save bar.
    pub(crate) holding: bool,
    pub(crate) held: bool,
    /// Someone tried to leave with unsaved edits: how many times, and when last.
    pub(crate) nudge: (u32, Option<Instant>),
    /// Closing: the screen fades and grows away, then goes.
    closing: Option<Instant>,
    focused_once: bool,
    pub(crate) state: crate::ui::settings_pages::PageState,
    pub(crate) servers: crate::ui::settings_servers::ServersForm,
    pub(crate) data: crate::ui::settings_data::DataForm,
    pub(crate) security: crate::ui::settings_security::SecurityForm,
    pub(crate) agents: crate::ui::settings_agents::AgentsForm,
    pub(crate) themes: crate::ui::settings_themes::ThemesForm,
    pub(crate) voice: crate::ui::settings_voice::VoiceForm,
    pub(crate) backup: crate::ui::settings_backup::BackupForm,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

/// How long the screen takes to come and go.
const OPENING: Duration = Duration::from_millis(280);

impl SettingsView {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let look = Look::new();
        // The Advanced page's counts change without the store changing: look again now and then.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(600)).await;
                let looked = this.update(cx, |this, cx| {
                    if this.page == Page::Advanced && crate::core::reports::pending() != this.pending {
                        cx.notify();
                    }
                });
                if looked.is_err() {
                    break;
                }
            }
        })
        .detach();
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.screen.search")));
        let subscriptions =
            vec![cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.pick_first(window, cx),
                _ => {}
            })];
        Self {
            core,
            page: Page::Appearance,
            keys: Default::default(),
            account: AccountForm::new(window, cx),
            look,
            pending: Default::default(),
            friends_error: None,
            sound_busy: None,
            sound_error: None,
            sliders: Sliders::default(),
            query,
            glow: None,
            scroll_to: None,
            places: Default::default(),
            scroll: ScrollHandle::new(),
            column: 792.0,
            wide: true,
            holding: false,
            held: false,
            nudge: (0, None),
            closing: None,
            focused_once: false,
            state: Default::default(),
            servers: Default::default(),
            data: Default::default(),
            security: Default::default(),
            agents: Default::default(),
            themes: Default::default(),
            voice: Default::default(),
            backup: Default::default(),
            _subscriptions: subscriptions,
        }
    }

    /// A note in the corner, over the settings.
    pub(crate) fn toast(&mut self, icon: &'static str, title: String, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::Toast { icon, title });
    }

    pub(crate) fn set(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Prefs)) {
        self.core.set_prefs(f);
        cx.emit(SettingsEvent::Prefs);
        cx.notify();
    }

    /// The instance the account pages are about, and you there.
    pub(crate) fn account_me(&mut self) -> Option<(String, pb::User)> {
        let key = self.account_key()?;
        let me = self.core.shared.read(|s| s.instance(&key).and_then(|i| i.me.clone()))?;
        Some((key, me))
    }

    /// The instance's name as the pages say it ("this instance" until it's known).
    pub(crate) fn place(&self, key: &str) -> String {
        self.core
            .shared
            .read(|s| s.instance(key).and_then(|i| i.node.as_ref().map(|n| n.name.clone())))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| t("settings.nav.thisInstance"))
    }

    fn groups(&mut self) -> Vec<Group> {
        let mut groups = vec![Group { label: Some(t("settings.nav.app")), sections: app_sections() }];
        let desktop = Group { label: Some(t("common.device.desktop")), sections: desktop_sections() };
        let Some((key, me)) = self.account_me() else {
            groups.push(desktop);
            return groups;
        };
        {
            let place = self.place(&key);
            let issuer = self.core.shared.read(|s| {
                s.instance(&key)
                    .and_then(|i| i.node.as_ref())
                    .and_then(|n| n.auth.as_ref())
                    .map(|a| a.linked_issuer.clone())
                    .unwrap_or_default()
            });
            groups.push(Group {
                label: Some(t_with("settings.nav.account", &[("instance", Arg::Str(&place))])),
                sections: account_sections(&me, &place, &issuer_name(&issuer)),
            });
            groups.push(desktop);
            let mut out =
                Section::new(Page::Session, "log-out", t("settings.nav.signOut"), String::new(), "log out remove");
            out.danger = true;
            groups.push(Group { label: None, sections: vec![out] });
        }
        groups
    }

    /// Opens a section (and, from search, one setting in it), unless unsaved edits hold the page.
    pub(crate) fn choose(&mut self, page: Page, setting: Option<&'static str>, cx: &mut Context<Self>) {
        if page != self.page && self.held {
            self.hold_on(cx);
            return;
        }
        if page != self.page {
            self.scroll.set_offset(point(px(0.0), px(0.0)));
            self.places.borrow_mut().clear();
        }
        self.page = page;
        if let Some(id) = setting {
            let n = self.glow.map_or(0, |(_, n)| n + 1);
            self.glow = Some((id, n));
            self.scroll_to = Some(id);
        }
        cx.notify();
    }

    /// Someone tried to leave with unsaved edits: the save bar shakes.
    pub(crate) fn hold_on(&mut self, cx: &mut Context<Self>) {
        self.nudge = (self.nudge.0 + 1, Some(Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1850)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The save bar's alarm, while someone just tried to leave.
    pub(crate) fn alarm(&self) -> Option<u32> {
        match self.nudge {
            (n, Some(at)) if at.elapsed() < Duration::from_millis(1800) => Some(n),
            _ => None,
        }
    }

    fn pick_first(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let groups = self.groups();
        let first = search(&groups, &query)
            .and_then(|results| results.first().map(|(s, settings)| (s.page, settings.first().map(|x| x.0))));
        if let Some((page, setting)) = first {
            self.choose(page, setting, cx);
        }
    }

    /// Escape: clears a search first, then closes (unless unsaved edits hold the screen).
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.query.read(cx).value().is_empty() {
            self.query.update(cx, |q, cx| q.set_value("", window, cx));
            cx.notify();
            return;
        }
        self.close(cx);
    }

    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        if self.held {
            self.hold_on(cx);
            return;
        }
        if self.closing.is_some() {
            return;
        }
        if cx.reduce_motion() {
            cx.emit(SettingsEvent::Close);
            return;
        }
        self.closing = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPENING).await;
            let _ = this.update(cx, |_, cx| cx.emit(SettingsEvent::Close));
        })
        .detach();
    }

    /// The section's own page.
    fn page_body(&mut self, prefs: &Prefs, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Appearance => self.appearance_page(prefs, p, window, cx),
            Page::Themes => self.themes_page(prefs, p, window, cx),
            Page::Backdrop => self.background_page(prefs, p, window, cx),
            Page::Accessibility => self.accessibility_page(prefs, p, window, cx),
            Page::Chat => self.chat_page(prefs, p, window, cx),
            Page::Language => self.language_page(prefs, p, window, cx),
            Page::Notifications => self.notifications_page(prefs, p, window, cx),
            Page::Voice => self.voice_page(prefs, p, window, cx),
            Page::Keybinds => self.keyboard_page(prefs, p, window, cx),
            Page::Streamer => self.streamer_page(prefs, p, window, cx),
            Page::Advanced => self.advanced_page(prefs, p, window, cx),
            Page::Instances => self.instances_page(prefs, p, cx),
            Page::Updates => self.updates_page(prefs, p, window, cx),
            Page::About => about_page(p),
            Page::Profile => self.profile_page(p, window, cx),
            Page::ServerProfiles => self.server_profiles_page(p, window, cx),
            Page::Devices => self.devices_page(p, window, cx),
            Page::SignIn => self.sign_in_page(p, window, cx),
            Page::Security => self.security_page(p, window, cx),
            Page::Password => self.password_page(p, window, cx),
            Page::Linked => self.linked_page(p, cx),
            Page::ServerNotifications => self.server_notifications_page(p, window, cx),
            Page::Agents => self.agents_page(p, window, cx),
            Page::Friends => self.friends_page(p, window, cx),
            Page::Privacy => self.privacy_page(p, window, cx),
            Page::Session => self.session_page(p, cx),
        }
    }

    /// Every instance added here, signed in or not (the desktop's account switcher).
    fn instances_page(&mut self, prefs: &Prefs, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let streamer = prefs.streamer_mode;
        let accounts = self.core.shared.read(|s| {
            s.order
                .iter()
                .filter_map(|k| s.instance(k))
                .map(|i| (i.key.clone(), i.name(), i.url.clone(), i.me.clone(), i.connection))
                .collect::<Vec<_>>()
        });
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, (key, name, url, me, connection)) in accounts.into_iter().enumerate() {
            let signed_out = connection == Connection::SignedOut;
            let who = me
                .as_ref()
                .filter(|_| !signed_out)
                .map(|m| if streamer { user_name(m) } else { format!("{} · @{}", user_name(m), m.username) });
            let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .pr(px(8.0))
                    .rounded(radius_2xl())
                    .bg(p.card)
                    .border_1()
                    .border_color(p.border)
                    .child(
                        div().relative().child(avatar(me.as_ref(), 44.0, p)).child(
                            div()
                                .absolute()
                                .right(px(-2.0))
                                .bottom(px(-2.0))
                                .child(conn_dot(connection, p).border_color(p.card)),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().font_weight(FontWeight::BOLD).child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(who.unwrap_or_else(|| t("workspace.connection.signedOut"))),
                            )
                            .when(!streamer, |el| el.child(div().text_xs().text_color(p.muted_foreground).child(url))),
                    )
                    .child(if signed_out {
                        primary_button(SharedString::from(format!("again-{key}")), t("connect.account.signIn"), p)
                            .h(px(32.0))
                            .text_sm()
                            .on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(SettingsEvent::SignIn { key: k1.clone() })),
                            )
                            .into_any_element()
                    } else {
                        soft_button(SharedString::from(format!("out-{key}")), t("accountsettings.shared.signOut"), p)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let core = this.core.clone();
                                let key = k2.clone();
                                drop(core.spawn({
                                    let core = core.clone();
                                    async move { core.sign_out(&key).await }
                                }));
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .child(icon_button(SharedString::from(format!("remove-{key}")), "trash", p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.core.remove_instance(&k3);
                            cx.notify();
                        },
                    ))),
                SharedString::from(format!("acct-{key}")),
                Duration::from_millis(40 * n as u64),
                8.0,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(list)
            .child(
                div().child(
                    crate::ui::settings_controls::button(
                        "add-instance",
                        t("desktop.settings.addInstance"),
                        Some("plus"),
                        crate::ui::settings_controls::Look::Outline,
                        false,
                        p,
                    )
                    .rounded(crate::ui::theme::radius_xl())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::AddInstance))),
                ),
            )
            .into_any_element()
    }

    /// Signing out of the instance on screen, or taking it off this computer (`Session`).
    fn session_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        use crate::ui::settings_controls::{Look, button};
        let Some(key) = self.account_key() else { return div().into_any_element() };
        let place = self.place(&key);
        let (k1, k2) = (key.clone(), key.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .p(px(16.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div().text_sm().font_weight(FontWeight::BOLD).child(t_with(
                                    "accountsettings.session.signOutOf",
                                    &[("instance", Arg::Str(&place))],
                                )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(t("accountsettings.session.signOutHint")),
                            ),
                    )
                    .child(
                        button(
                            "session-out",
                            t("accountsettings.shared.signOut"),
                            Some("log-out"),
                            Look::Outline,
                            false,
                            p,
                        )
                        .rounded(crate::ui::theme::radius_xl())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let core = this.core.clone();
                            let key = k1.clone();
                            drop(core.spawn({
                                let core = core.clone();
                                async move { core.sign_out(&key).await }
                            }));
                            this.close(cx);
                        })),
                    ),
                "session-a",
                Duration::ZERO,
                10.0,
            ))
            .child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(alpha(p.destructive, 0.4))
                    .bg(alpha(p.destructive, 0.05))
                    .p(px(16.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(p.destructive)
                                    .child(t("desktop.settings.removeHere")),
                            )
                            .child(div().text_xs().text_color(p.muted_foreground).child(t_with(
                                "accountsettings.session.removeHint",
                                &[("instance", Arg::Str(&place))],
                            ))),
                    )
                    .child(
                        button(
                            "session-remove",
                            t("accountsettings.session.removeButton"),
                            None,
                            Look::Destructive,
                            false,
                            p,
                        )
                        .rounded(crate::ui::theme::radius_xl())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.core.remove_instance(&k2);
                            this.close(cx);
                        })),
                    ),
                "session-b",
                Duration::from_millis(40),
                10.0,
            ))
            .into_any_element()
    }

    /// The side menu: the title, the search, then the groups (or what the search found).
    fn menu(&mut self, groups: &[Group], p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let query = self.query.read(cx).value().to_string();
        let subtitle = match self.account_me() {
            Some((key, _)) => t_with("settings.nav.signedIn", &[("instance", Arg::Str(&self.place(&key)))]),
            None => t("settings.nav.thisDevice"),
        };
        let focused = gpui_kit::Focusable::focus_handle(self.query.read(cx), cx).is_focused(window);
        let search_box = div()
            .relative()
            .h(px(36.0))
            .rounded(radius_lg())
            .border_1()
            .border_color(if focused { alpha(p.primary, 0.5) } else { alpha(p.border, 0.0) })
            .bg(if focused { p.background.into() } else { alpha(p.muted, 0.7) })
            .when(focused, |el| {
                el.shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.1),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(4.0),
                    inset: false,
                }])
            })
            .flex()
            .items_center()
            .pl(px(32.0))
            .pr(px(32.0))
            .text_sm()
            .child(
                div()
                    .absolute()
                    .left(px(9.0))
                    .top(px(9.0))
                    .text_color(if focused { p.primary } else { p.muted_foreground })
                    .child(icon("search").size(px(16.0))),
            )
            .child(div().flex_1().min_w_0().child(Input::new(&self.query).appearance(false)))
            .when(!query.is_empty(), |el| {
                let (hover, fg) = (p.muted, p.foreground);
                el.child(
                    div()
                        .id("settings-search-clear")
                        .absolute()
                        .right(px(5.0))
                        .top(px(5.0))
                        .size(px(24.0))
                        .rounded(radius_md())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.query.update(cx, |q, cx| q.set_value("", window, cx));
                            cx.notify();
                        }))
                        .child(icon("x").size(px(14.0))),
                )
            });

        let mut nav = div()
            .w(px(240.0))
            .flex_none()
            .pt(px(64.0))
            .pb(px(64.0))
            .pr(px(12.0))
            .pl(px(20.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .px(px(8.0))
                    .child(
                        div()
                            .text_lg()
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .truncate()
                            .child(t("settings.screen.title")),
                    )
                    .child(
                        div().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).truncate().child(subtitle),
                    ),
            )
            .child(search_box);

        if let Some(results) = search(groups, &query) {
            nav = nav.child(self.results(&results, &query, p, cx));
            return nav.into_any_element();
        }

        // The highlight glides between rows: where the chosen one sits in the list.
        let mut start = 0.0;
        let mut at = None;
        let mut list = div().relative().flex().flex_col().gap(px(20.0));
        let mut rows_list = Vec::new();
        let mut n = 0usize;
        for (g, group) in groups.iter().enumerate() {
            let mut block = div().flex().flex_col().gap(px(2.0));
            let mut y = start;
            if g > 0 {
                block = block.border_t_1().border_color(alpha(p.border, 0.7)).pt(px(12.0));
                y += 13.0;
            }
            if let Some(label) = &group.label {
                block = block.child(
                    div()
                        .mb(px(4.0))
                        .px(px(8.0))
                        .text_size(px(11.2))
                        .line_height(px(16.8))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(tracked(label.to_uppercase(), WIDE)),
                );
                y += 16.8 + 4.0 + 2.0;
            }
            for (i, section) in group.sections.iter().enumerate() {
                if i > 0 {
                    y += 2.0;
                }
                let active = section.page == self.page;
                if active {
                    at = Some((y, section.danger));
                }
                let page = section.page;
                let trailing = group.label.is_none();
                let (hover_bg, hover_fg) = if section.danger {
                    (alpha(p.destructive, 0.1), p.destructive)
                } else {
                    (alpha(p.muted, 0.7), p.foreground)
                };
                let color = match (active, section.danger) {
                    (true, true) => p.destructive.into(),
                    (true, false) => p.primary.into(),
                    (false, true) => alpha(p.destructive, 0.8),
                    (false, false) => p.muted_foreground.into(),
                };
                let row = div()
                    .id(SharedString::from(format!("menu-{page:?}")))
                    .relative()
                    .h(px(32.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(radius_lg())
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .cursor_pointer()
                    .when(!active, |el| el.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.blur(cx);
                        this.choose(page, None, cx)
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(section.label.clone()))
                    .when(trailing, |el| el.child(icon(section.glyph).size(px(16.0))));
                block = block.child(slide(row, SharedString::from(format!("menu-in-{page:?}")), n));
                n += 1;
                y += 32.0;
            }
            start = y + 20.0;
            rows_list.push(block);
        }
        if let Some((top, danger)) = at {
            let top = motion::follow("settings-hl", top, window, cx);
            list = list.child(
                div().absolute().left_0().right_0().top(px(top)).h(px(32.0)).rounded(radius_lg()).bg(if danger {
                    alpha(p.destructive, 0.12)
                } else {
                    alpha(p.primary, 0.15)
                }),
            );
        }
        list = list.children(rows_list);
        nav.child(list).into_any_element()
    }

    /// What a search found: sections, and under each the single settings that matched.
    fn results(&mut self, results: &[Found<'_>], query: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if results.is_empty() {
            return motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(24.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(motion::once(
                        div().child(icon("search-x").size(px(28.0))),
                        SharedString::from(format!("nomatch-{query}")),
                        Duration::from_millis(700),
                        |el, t| {
                            let k = if t < 1.0 / 7.0 { 0.0 } else { (t - 1.0 / 7.0) * 7.0 / 6.0 };
                            let wiggle = (k * std::f32::consts::TAU * 2.0).sin() * (1.0 - k) * 2.0;
                            el.relative().left(px(wiggle))
                        },
                    ))
                    .child(t_with("settings.screen.noMatches", &[("query", Arg::Str(query))])),
                "settings-nomatch",
                Duration::ZERO,
                8.0,
            )
            .into_any_element();
        }
        let mut list = div().flex().flex_col().gap(px(2.0));
        let mut n = 0;
        for (section, settings) in results {
            let page = section.page;
            let hover = alpha(p.muted, 0.7);
            list = list.child(slide(
                div()
                    .id(SharedString::from(format!("result-{page:?}")))
                    .h(px(32.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if section.danger { alpha(p.destructive, 0.8) } else { p.foreground.into() })
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.blur(cx);
                        this.choose(page, None, cx)
                    }))
                    .child(icon(section.glyph).size(px(16.0)))
                    .child(div().truncate().child(section.label.clone())),
                SharedString::from(format!("result-in-{page:?}")),
                n,
            ));
            n += 1;
            for (id, label, _) in settings.iter().copied() {
                let id: &'static str = id;
                let fg = p.foreground;
                list = list.child(slide(
                    div()
                        .id(SharedString::from(format!("result-{page:?}-{id}")))
                        .h(px(30.0))
                        .pl(px(24.0))
                        .pr(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_lg())
                        .text_size(px(12.8))
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(page, Some(id), cx)))
                        .child(div().opacity(0.6).child(icon("corner-down-right").size(px(14.0))))
                        .child(div().truncate().child(label.clone())),
                    SharedString::from(format!("result-in-{page:?}-{id}")),
                    n,
                ));
                n += 1;
            }
        }
        list.into_any_element()
    }
}

/// A menu row sliding in from the left, a beat after the one above it.
fn slide(el: impl IntoElement + gpui_kit::Styled + 'static, id: SharedString, n: usize) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let delay = 0.05 + n as f32 * 0.03;
    let total = Duration::from_secs_f32(delay + 0.4);
    let start = delay / total.as_secs_f32();
    el.with_animation(id, Animation::new(total), move |el, t| {
        let k = if t <= start { 0.0 } else { ((t - start) / (1.0 - start)).clamp(0.0, 1.0) };
        let e = 1.0 - (1.0 - k).powi(3);
        el.opacity(e).left(px(-10.0 * (1.0 - e)))
    })
    .into_any_element()
}

/// The app's version and who made it.
fn about_page(p: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div().flex().items_center().gap(px(14.0)).child(crate::ui::widgets::fuwa_mark(56.0, p)).child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(t("common.device.desktop")))
                    .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                        "desktop.settings.version",
                        &[("version", Arg::Str(env!("CARGO_PKG_VERSION")))],
                    ))),
            ),
        )
        .child(div().text_sm().text_color(p.muted_foreground).child(t("desktop.settings.credits")))
        .child(
            div()
                .flex()
                .gap(px(10.0))
                .child(
                    soft_button("source", t("desktop.settings.sourceCode"), p)
                        .on_click(|_, _, cx| cx.open_url("https://github.com/waifu-devs/fuwa")),
                )
                .child(
                    soft_button("site", "waifu.dev", p)
                        .on_click(|_, _, cx| cx.open_url("https://www.waifu.dev/projects")),
                ),
        )
        .into_any_element()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // As the web's dialog does, the search takes the keys when settings open.
        if !self.focused_once {
            self.focused_once = true;
            self.query.update(cx, |q, cx| q.focus(window, cx));
        }
        let p = pal(cx);
        let prefs = self.core.prefs();
        let behind = crate::ui::backdrop::layers(&crate::ui::theme::backdrop(cx), &p, window, cx);

        // The web's layout: the menu takes 15rem and half of what's left past 67rem; the
        // section's column is at most 60rem, less the close button's 4rem and its padding.
        let width = f32::from(window.viewport_size().width);
        let aside = 240.0 + ((width - 1072.0).max(0.0) / 2.0);
        let main = (width - aside).max(320.0);
        let inner = main.min(960.0);
        self.column = inner - 64.0 - 80.0;
        self.wide = width >= 1280.0;

        let groups = self.groups();
        // A page that went away (signed out, a password page on a linked account) falls back.
        if !groups.iter().any(|g| g.sections.iter().any(|s| s.page == self.page)) {
            self.page = Page::Appearance;
        }
        let menu = self.menu(&groups, &p, window, cx);
        let (label, about, danger) = groups
            .iter()
            .flat_map(|g| g.sections.iter())
            .find(|s| s.page == self.page)
            .map(|s| (s.label.clone(), s.about.clone(), s.danger))
            .unwrap_or_default();

        self.open_pending_edit(window, cx);
        self.held = self.holding;
        self.holding = false;
        let body = self.page_body(&prefs, &p, window, cx);

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
                    self.scroll.set_offset(point(px(0.0), px(-target.clamp(0.0, most.max(0.0)))));
                    self.scroll_to = None;
                }
                None => window.request_animation_frame(),
            }
        }

        let header = div()
            .mb(px(24.0))
            .child(
                div()
                    .text_2xl()
                    .line_height(px(32.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .when(danger, |el| el.text_color(p.destructive))
                    .child(tracked(label, TIGHT)),
            )
            .when(!about.is_empty(), |el| {
                el.child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(about))
            });
        let content =
            div().w(px(inner - 64.0)).flex_none().px(px(40.0)).pt(px(64.0)).pb(px(16.0)).flex().flex_col().child(
                motion::rise(
                    div().flex().flex_col().child(header).child(body),
                    SharedString::from(format!("page-{:?}", self.page)),
                    Duration::ZERO,
                    14.0,
                ),
            );

        let (hover_bg, hover_fg, hover_ring) = (p.muted, p.foreground, alpha(p.foreground, 0.4));
        let close = div()
            .id("settings-close")
            .absolute()
            .top(px(64.0))
            .left(px(aside + inner - 24.0 - 40.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .id("settings-close-ring")
                    .size(px(40.0))
                    .rounded_full()
                    .border_2()
                    .border_color(p.border)
                    .text_color(p.muted_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |s| s.bg(hover_bg).text_color(hover_fg).border_color(hover_ring))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("x").size(px(20.0))),
            )
            .child(div().text_size(px(10.4)).font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"));

        let screen = div()
            .id("settings")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .bg(p.background)
            .text_color(p.foreground)
            .when_some(behind, |el, behind| el.child(behind))
            .child(
                div()
                    .id("settings-menu")
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
                    .id("settings-body")
                    .flex_1()
                    .h_full()
                    .bg(p.chat_surface)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .child(close)
            .when_some(self.state.overlay.take(), |el, overlay| el.child(overlay));

        // It comes in from a little larger, fading up, and goes the same way.
        use gpui_kit::{Animation, AnimationExt as _};
        let (id, out) = match self.closing {
            Some(at) => (SharedString::from(format!("settings-out-{at:?}")), true),
            None => (SharedString::from("settings-in"), false),
        };
        let (w, h) = (width, f32::from(window.viewport_size().height));
        screen.with_animation(id, Animation::new(OPENING).with_easing(gpui_kit::ease_out_quint()), move |el, t| {
            let k = if out { 1.0 - t } else { t };
            let (dx, dy) = (w * 0.02 * (1.0 - k), h * 0.02 * (1.0 - k));
            el.opacity(k).top(px(-dy)).bottom(px(-dy)).left(px(-dx)).right(px(-dx))
        })
    }
}

// ───────────────────────── Older helpers, still used by a few pages ─────────────────────────

pub(crate) fn section(title: &str, body: impl IntoElement, p: &Palette) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(11.2))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(body)
}

pub(crate) fn toggle_row(
    id: &'static str,
    title: &str,
    body: &str,
    on: bool,
    p: &Palette,
    cx: &mut Context<SettingsView>,
    set: impl Fn(&mut SettingsView, bool, &mut Context<SettingsView>) + 'static,
) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let set = std::rc::Rc::new(set);
    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .p(px(16.0))
        .rounded(corner(16.0))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().font_weight(FontWeight::BOLD).child(title.to_owned()))
                .child(div().text_sm().text_color(p.muted_foreground).child(body.to_owned())),
        )
        .child(div().flex_none().child(Switch::new(id).checked(on).on_change(move |checked, _, cx| {
            let (set, checked) = (set.clone(), *checked);
            let _ = entity.update(cx, |this, cx| set(this, checked, cx));
        })))
}
#[allow(dead_code)]
pub(crate) fn radio(on: bool, p: &Palette) -> impl IntoElement {
    div()
        .size(px(16.0))
        .rounded_full()
        .border_2()
        .border_color(if on { p.primary } else { p.muted_foreground })
        .flex()
        .items_center()
        .justify_center()
        .when(on, |el| el.child(div().size(px(6.0)).rounded_full().bg(p.primary)))
}
