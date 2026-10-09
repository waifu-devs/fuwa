//! A server's settings, full screen with a side menu like the app's own:
//! its name, picture and words, the welcome screen, invites, roles, emoji,
//! agents and webhooks, members, bans, AutoMod and the audit log.
//! Each page shows only to people whose permissions open it, as in the web
//! app's `ServerSettingsDialog.tsx`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, hsla, point, px,
};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::moderation::{Action, timed_out_until};
use crate::core::server_admin::{People, ServerPatch};
use crate::core::store::user_name;
use crate::pb::{self, AuditAction as A, Permission as P};
use crate::ui::moderate::{duration, stamp};
use crate::ui::motion;
use crate::ui::settings_controls::Look;
use crate::ui::text::{TIGHT, tracked};
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::widgets::{
    app_badge, avatar, error_line, icon, icon_button, icon_button_in, is_agent, labeled, pal, server_icon,
};
use gpui_kit::{Div, Stateful};

mod agents;
mod applications;
mod audit;
mod automod;
mod channels;
mod emoji;
mod frame;
mod joinform;
mod menu;
mod onboarding;
mod overview;
mod pages;
mod people;
mod recordings;
pub(crate) mod roles;
pub(crate) use roles::switch;
mod shared;
mod sso;
mod stage;
mod usage;
mod webhooks;
mod welcome;

/// The web's `<Button className="rounded-xl font-bold">` as these pages use it: `h-9`, `text-sm`,
/// and any icon given after the words drawn before them, as lucide icons sit in the web's buttons.
fn web_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    look: Look,
    p: &Palette,
) -> Stateful<Div> {
    crate::ui::settings_controls::button(id, label, None, look, false, p)
        .rounded(crate::ui::theme::radius_xl())
        .font_weight(FontWeight::BOLD)
        .flex_row_reverse()
}

pub(crate) fn primary_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    p: &Palette,
) -> Stateful<Div> {
    web_button(id, label, Look::Primary, p)
}

pub(crate) fn danger_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    p: &Palette,
) -> Stateful<Div> {
    web_button(id, label, Look::Destructive, p)
}

/// The web's outline buttons.
pub(crate) fn soft_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    p: &Palette,
) -> Stateful<Div> {
    web_button(id, label, Look::Outline, p)
}

pub enum ServerSettingsEvent {
    Close,
    /// Open the moderation dialog over the settings.
    Moderate {
        user_id: String,
        action: Action,
    },
    /// Open the new-channel dialog over the settings, in a category or none.
    CreateChannel {
        parent: String,
    },
    /// Open the invite dialog over the settings, to the server (empty) or a channel.
    Invite {
        channel: String,
    },
    /// Open the app's own settings at Agents (where people make theirs).
    OpenAgents,
    /// Say something for a moment, over the settings.
    Toast {
        icon: &'static str,
        title: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Page {
    Overview,
    Access,
    Sso,
    JoinForm,
    Welcome,
    Invites,
    Roles,
    Channels,
    Emoji,
    ProfileItems,
    Integrations,
    Shared,
    Recordings,
    Usage,
    Limits,
    Applications,
    Members,
    Bans,
    AutoMod,
    AuditLog,
    Ownership,
    Danger,
}

/// Every page, in the web's menu order (`serverSettingsTabs.ts`).
const ALL: [Page; 22] = [
    Page::Overview,
    Page::Access,
    Page::Sso,
    Page::JoinForm,
    Page::Welcome,
    Page::Invites,
    Page::Roles,
    Page::Channels,
    Page::Emoji,
    Page::ProfileItems,
    Page::Integrations,
    Page::Shared,
    Page::Recordings,
    Page::Usage,
    Page::Limits,
    Page::Applications,
    Page::Members,
    Page::Bans,
    Page::AutoMod,
    Page::AuditLog,
    Page::Ownership,
    Page::Danger,
];

impl Page {
    /// Whether someone with this access (and whether they run the instance) may open it.
    fn open_to(self, access: &crate::core::permissions::Access, admin: bool) -> bool {
        let manage = access.has(P::ManageServer);
        match self {
            Page::Overview | Page::Access | Page::JoinForm | Page::Welcome | Page::Shared | Page::Recordings => manage,
            Page::AutoMod => manage,
            Page::Sso | Page::Ownership => access.owner,
            Page::Invites => {
                manage
                    || access.has(P::CreateInvite)
                    || access.channels.keys().any(|c| access.has_in(c, P::CreateInvite))
            }
            Page::Roles => access.has(P::ManageRoles),
            Page::Channels => {
                access.channels.keys().any(|c| access.has_in(c, P::ManageChannels) || access.has_in(c, P::ManageRoles))
            }
            Page::Emoji => access.has(P::ManageEmoji),
            Page::ProfileItems => manage,
            Page::Integrations => access.has(P::ManageWebhooks) || manage,
            Page::Usage => admin || manage,
            Page::Limits => admin,
            Page::Applications => access.has(P::KickMembers),
            Page::Members => [P::ManageRoles, P::ManageNicknames, P::KickMembers, P::BanMembers, P::TimeOutMembers]
                .into_iter()
                .any(|p| access.has(p)),
            Page::Bans => access.has(P::BanMembers),
            Page::AuditLog => access.has(P::ViewAuditLog),
            Page::Danger => access.owner || admin,
        }
    }

    fn label(self) -> String {
        t(&format!("serversettings.nav.{}", self.key()))
    }

    fn glyph(self) -> &'static str {
        match self {
            Page::Overview => "settings",
            Page::Access => "door-open",
            Page::Sso => "building",
            Page::JoinForm => "clipboard-list",
            Page::Welcome => "party-popper",
            Page::Invites => "link",
            Page::Roles => "shield",
            Page::Channels => "hash",
            Page::Emoji => "face-slightly-smiling-plus",
            Page::ProfileItems => "sparkles",
            Page::Integrations => "webhook",
            Page::Shared => "link-2",
            Page::Recordings => "video",
            Page::Usage => "chart-column",
            Page::Limits => "gauge",
            Page::Applications => "inbox",
            Page::Members => "users",
            Page::Bans => "gavel",
            Page::AutoMod => "bot",
            Page::AuditLog => "scroll-text",
            Page::Ownership => "crown",
            Page::Danger => "trash",
        }
    }

    /// The line under the page's heading.
    fn about(self) -> String {
        match self {
            Page::Ownership | Page::Danger => String::new(),
            _ => t(&format!("serversettings.nav.{}About", self.key())),
        }
    }

    /// The page's name in the translations (`serversettings.nav.<key>`).
    fn key(self) -> &'static str {
        match self {
            Page::Overview => "overview",
            Page::Access => "access",
            Page::Sso => "sso",
            Page::JoinForm => "joinForm",
            Page::Welcome => "welcome",
            Page::Invites => "invites",
            Page::Roles => "roles",
            Page::Channels => "channels",
            Page::Emoji => "emoji",
            Page::ProfileItems => "profileItems",
            Page::Integrations => "integrations",
            Page::Shared => "shared",
            Page::Recordings => "recordings",
            Page::Usage => "usage",
            Page::Limits => "limits",
            Page::Applications => "applications",
            Page::Members => "members",
            Page::Bans => "bans",
            Page::AutoMod => "automod",
            Page::AuditLog => "auditLog",
            Page::Ownership => "ownership",
            Page::Danger => "danger",
        }
    }

    fn danger(self) -> bool {
        matches!(self, Page::Ownership | Page::Danger)
    }

    /// Which group of the menu it's in: the server's, the people's, or the dangerous ones.
    fn group(self) -> usize {
        match self {
            Page::Applications | Page::Members | Page::Bans | Page::AutoMod | Page::AuditLog => 1,
            Page::Ownership | Page::Danger => 2,
            _ => 0,
        }
    }

    /// More words search finds it by (the web's `keywords`).
    fn keywords(self) -> &'static str {
        match self {
            Page::Access => "join public private lock",
            Page::Sso => {
                "sso saml oidc openid okta entra azure google workspace keycloak authentik identity provider organization company"
            }
            Page::JoinForm => "rules screening agree questions application form onboarding",
            Page::Welcome => {
                "welcome onboarding new members greet suggested channels banner header cover accent color interests"
            }
            Page::Invites => "invite link code revoke expire uses",
            Page::Roles => "permissions admin moderator rank color hoist mention everyone",
            Page::Channels => "reorder drag category topic slowmode slow mode private permissions overwrites",
            Page::Emoji => "emoji emote custom sticker upload",
            Page::ProfileItems => "profile items effects decorations avatar frame sparkle wear",
            Page::Integrations => {
                "webhook webhooks integration apps bot bots agent agents ci github feed rss alerts post api discord"
            }
            Page::Shared => "share connect slack connect other server guest home code external partner",
            Page::Recordings => "record recording call voice video camera screen webm quality resolution fps",
            Page::Usage => "storage members messages",
            Page::Limits => "caps members channels storage",
            Page::Applications => "apply review approve reject let in turn down pending waiting",
            Page::Members => "admin role kick ban timeout nickname",
            Page::Bans => "unban banned",
            Page::AutoMod => {
                "automod auto moderation filter blocked words banned words swear profanity spam mentions pings raid links urls block alert time out ai smart jev clef typesafe cloudflare hate scam"
            }
            Page::AuditLog => "history log moderation",
            Page::Ownership => "owner hand give",
            Page::Danger => "remove",
            Page::Overview => "",
        }
    }

    /// Single settings on the page search can jump to: (id, label, keywords).
    fn settings(self) -> Vec<(&'static str, String, &'static str)> {
        let s = |id, key: &str, words| (id, t(key), words);
        match self {
            Page::Overview => vec![
                s("name", "serversettings.nav.serverName", ""),
                s("icon", "serversettings.nav.serverIcon", "picture image upload logo avatar"),
                s("description", "serversettings.nav.description", ""),
                s("join-messages", "serversettings.nav.joinMessages", "system channel welcome greet"),
                s("default-notifications", "serversettings.nav.defaultNotifications", "mentions ping"),
            ],
            Page::Access => vec![
                s("discoverable", "serversettings.nav.discoverable", "discoverable public hidden invite only"),
                s(
                    "applications",
                    "serversettings.nav.applyToJoin",
                    "applications review approve screening vetting questions",
                ),
                s("linked-only", "serversettings.nav.linkedOnly", "linked verified account sign in"),
                s("account-age", "serversettings.nav.accountAge", "new accounts spam raid verification"),
            ],
            Page::Sso => vec![
                s("sso-protocol", "serversettings.nav.ssoProtocol", "saml oidc openid"),
                s("sso-required", "serversettings.nav.ssoRequired", "sso members join"),
                s("sso-recheck", "serversettings.nav.ssoRecheck", "sso recheck expire days"),
                s("sso-domains", "serversettings.nav.ssoDomains", "sso allowed"),
            ],
            Page::JoinForm => vec![
                s("rules", "serversettings.nav.rules", "screening agree code of conduct"),
                s("questions", "serversettings.nav.questions", "apply form"),
            ],
            Page::Welcome => vec![
                s("banner-picture", "settings.nav.banner", "header cover picture image"),
                s("banner-focus", "serversettings.nav.bannerFocus", "crop position"),
                s("accent-color", "serversettings.nav.accentColor", "colour tint theme"),
                s("welcome-enabled", "serversettings.nav.welcomeEnabled", ""),
                s("welcome-description", "serversettings.nav.welcomeMessage", "description"),
                s("welcome-channels", "serversettings.nav.suggestedChannels", "start here"),
                s("onboarding-enabled", "serversettings.nav.onboarding", "steps interests"),
                s(
                    "onboarding-steps",
                    "serversettings.nav.onboardingSteps",
                    "pick interests roles channels rules hello",
                ),
            ],
            Page::Roles => vec![
                s("role-permissions", "serversettings.nav.rolePermissions", "administrator manage"),
                s("role-members", "serversettings.nav.roleMembers", "assign give"),
            ],
            Page::Channels => vec![
                s("slowmode", "serversettings.nav.slowmode", "slowmode rate limit"),
                s("channel-permissions", "serversettings.nav.channelPermissions", "private hidden access roles"),
            ],
            Page::Integrations => vec![
                s("agents", "settings.nav.agents", "bot add username"),
                s("webhooks", "serversettings.nav.webhooks", "address url token"),
            ],
            Page::Recordings => vec![
                s(
                    "camera-quality",
                    "serversettings.camera.title",
                    "camera resolution frame rate fps ceiling 1080p 720p",
                ),
                s("record-video", "serversettings.nav.recordVideo", "camera screen share webm"),
            ],
            _ => Vec::new(),
        }
    }
}

/// The pages someone with this access may open, in menu order.
fn pages(access: &crate::core::permissions::Access, admin: bool) -> Vec<Page> {
    ALL.into_iter().filter(|pg| pg.open_to(access, admin)).collect()
}

/// Whether someone with this access gets server settings at all.
pub fn can_open(access: &crate::core::permissions::Access) -> bool {
    !pages(access, false).is_empty()
}

/// A page and the settings on it a search found.
type Found = (Page, Vec<(&'static str, String, &'static str)>);

/// Pages and single settings whose words hold every word typed (the web's `search`).
fn search(shown: &[Page], query: &str) -> Option<Vec<Found>> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return None;
    }
    let hits = |hay: &str| {
        let hay = hay.to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    };
    let mut out = Vec::new();
    for &page in shown {
        let own = format!("{} {} {}", page.label(), page.about(), page.keywords());
        let settings: Vec<_> =
            page.settings().into_iter().filter(|s| hits(&format!("{} {} {}", s.1, s.2, page.label()))).collect();
        if !settings.is_empty() || hits(&own) {
            out.push((page, settings));
        }
    }
    Some(out)
}

pub struct ServerSettingsView {
    /// The Profile items page, made when first opened.
    profile_items: Option<Entity<crate::ui::profile_items::ProfileItemsView>>,
    core: Arc<Core>,
    pub key: String,
    pub server: String,
    page: Option<Page>,
    name: Entity<InputState>,
    description: Entity<TextareaState>,
    member_query: Entity<InputState>,
    /// Which server the fields were filled from, so they fill once.
    filled: bool,
    error: Option<String>,
    saved: Option<Instant>,
    invites: Option<(Vec<pb::Invite>, People)>,
    bans: Option<(Vec<pb::Ban>, People)>,
    audit: Option<Vec<pb::AuditEntry>>,
    audit_people: People,
    audit_more: bool,
    audit_loading: bool,
    audit_action: A,
    audit_open: Option<String>,
    copied: Option<(String, Instant)>,
    roles: roles::Roles,
    emojis: emoji::Emojis,
    hooks: webhooks::Hooks,
    agents: agents::Agents,
    automod: automod::AutoMod,
    channels: channels::Channels,
    welcome: welcome::Welcome,
    onboard: onboarding::Onboard,
    shared: shared::Shared,
    recordings: recordings::Recordings,
    /// A floating bar of changes not saved yet, drawn over the page's foot.
    bar: Option<AnyElement>,
    /// The menu's search.
    query: Entity<InputState>,
    focused_once: bool,
    /// The setting search picked, glowing where it landed (and a count, so it glows again).
    glow: Option<(&'static str, u32)>,
    /// A setting to scroll to once its page is on screen.
    scroll_to: Option<&'static str>,
    /// When search asked for `scroll_to`, to give up on one that never shows.
    scroll_since: Option<Instant>,
    /// Where each search target was drawn, for scrolling to it.
    places: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    /// The width the page's column has, and whether a preview fits beside a form.
    column: f32,
    wide: bool,
    /// Unsaved edits on the page on screen (a save bar was drawn); leaving shakes it.
    held: bool,
    /// Someone tried to leave with unsaved edits: how many times, and when last.
    nudge: (u32, Option<Instant>),
    /// Closing: the screen fades and grows away, then goes.
    closing: Option<Instant>,
    pages: pages::Pages,
    /// A picture being framed before it's uploaded (the icon, the banner, a webhook's).
    cropper: Option<crate::ui::cropper::CropSlot>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ServerSettingsEvent> for ServerSettingsView {}

impl ServerSettingsView {
    pub fn new(core: Arc<Core>, key: String, server: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(t("desktop.server.namePlaceholder")));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 8).placeholder(t("desktop.server.descriptionPlaceholder"))
        });
        let member_query = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.members.search")));
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.screen.search")));
        let (pages, page_subscriptions) = pages::Pages::new(window, cx);
        // The page reads the instance as it draws, so it draws again when that changes.
        let mut changes = core.changes();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let (roles, role_subscriptions) = roles::Roles::new(window, cx);
        let (emojis, emoji_subscriptions) = emoji::Emojis::new(window, cx);
        let (hooks, hook_subscriptions) = webhooks::Hooks::new(window, cx);
        let (agents, agent_subscriptions) = agents::Agents::new(window, cx);
        let (automod, automod_subscriptions) = automod::AutoMod::new(window, cx);
        let (welcome, welcome_subscriptions) = welcome::Welcome::new(window, cx);
        let (onboard, onboard_subscriptions) = onboarding::Onboard::new(window, cx);
        let (channels, channel_subscriptions) = channels::Channels::new(window, cx);
        let (shared, shared_subscriptions) = shared::Shared::new(window, cx);
        let mut subscriptions = vec![
            cx.subscribe(&name, |_: &mut Self, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
            cx.subscribe(&description, |_: &mut Self, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
            cx.subscribe(&member_query, |_: &mut Self, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
            cx.subscribe(&query, |_: &mut Self, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            }),
        ];
        subscriptions.extend(page_subscriptions);
        subscriptions.extend(role_subscriptions);
        subscriptions.extend(emoji_subscriptions);
        subscriptions.extend(hook_subscriptions);
        subscriptions.extend(agent_subscriptions);
        subscriptions.extend(automod_subscriptions);
        subscriptions.extend(welcome_subscriptions);
        subscriptions.extend(onboard_subscriptions);
        subscriptions.extend(channel_subscriptions);
        subscriptions.extend(shared_subscriptions);
        Self {
            profile_items: None,
            cropper: None,
            core,
            key,
            server,
            page: None,
            name,
            description,
            member_query,
            filled: false,
            error: None,
            saved: None,
            invites: None,
            bans: None,
            audit: None,
            audit_people: People::new(),
            audit_more: false,
            audit_loading: false,
            audit_action: A::Unspecified,
            audit_open: None,
            copied: None,
            roles,
            emojis,
            hooks,
            agents,
            automod,
            welcome,
            onboard,
            channels,
            shared,
            recordings: Default::default(),
            bar: None,
            query,
            focused_once: false,
            glow: None,
            scroll_to: None,
            scroll_since: None,
            places: Rc::new(RefCell::new(HashMap::new())),
            scroll: ScrollHandle::new(),
            column: 792.0,
            wide: true,
            held: false,
            nudge: (0, None),
            closing: None,
            pages,
            _subscriptions: subscriptions,
        }
    }

    /// Runs `future` on the core and hands its result back to the view.
    fn run<T: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = rx.await {
                let _ = this.update(cx, |this, cx| done(this, value, cx));
            }
        })
        .detach();
    }

    fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = Some(page);
        self.error = None;
        self.saved = None;
        match page {
            Page::Invites => self.load_invites(cx),
            Page::Bans => self.load_bans(cx),
            Page::AuditLog => self.load_audit(false, cx),
            Page::Recordings => {
                self.recordings.draft = None;
                self.recordings.camera = None;
            }
            _ => {}
        }
        cx.notify();
    }

    fn load_invites(&mut self, cx: &mut Context<Self>) {
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.invites(&key, &sid).await }, |this, result, cx| {
            match result {
                Ok(list) => this.invites = Some(list),
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
    }

    fn load_bans(&mut self, cx: &mut Context<Self>) {
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.bans(&key, &sid).await }, |this, result, cx| {
            match result {
                Ok(list) => this.bans = Some(list),
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
    }

    /// The newest page of the log, or the one after what's shown with `older`.
    fn load_audit(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.audit_loading {
            return;
        }
        let before = if older {
            self.audit.as_ref().and_then(|l| l.last()).map(|e| e.id.clone()).unwrap_or_default()
        } else {
            self.audit = None;
            String::new()
        };
        self.audit_loading = true;
        let (core, key, sid, action) = (self.core.clone(), self.key.clone(), self.server.clone(), self.audit_action);
        let actor = self.pages.people.audit_actor.clone();
        self.run(
            cx,
            async move { core.audit_log(&key, &sid, &before, &actor, action).await },
            move |this, result, cx| {
                this.audit_loading = false;
                match result {
                    Ok((entries, people, more)) => {
                        this.audit_people.extend(people);
                        this.audit_more = more;
                        match (&mut this.audit, older) {
                            (Some(list), true) => list.extend(entries),
                            _ => this.audit = Some(entries),
                        }
                    }
                    Err(err) => this.error = Some(err.message),
                }
                cx.notify();
            },
        );
    }

    /// Says "Saved" for a moment.
    fn flash_saved(&mut self, cx: &mut Context<Self>) {
        self.saved = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

impl Render for ServerSettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // As the web's dialog does, the search takes the keys when settings open.
        if !self.focused_once {
            self.focused_once = true;
            self.query.update(cx, |q, cx| q.focus(window, cx));
        }
        let p = pal(cx);
        let (server, access, admin) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (
                i.and_then(|i| i.server(&self.server).cloned()),
                i.map(|i| i.access(&self.server)).unwrap_or_default(),
                i.is_some_and(|i| i.admin),
            )
        });
        let Some(server) = server else {
            // The server went away (left, kicked or deleted).
            cx.defer_in(window, |_, _, cx| cx.emit(ServerSettingsEvent::Close));
            return div().into_any_element();
        };
        let mut allowed = pages(&access, admin);
        // Nor anything to offer before profile items.
        if !self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.has("profile-items"))) {
            allowed.retain(|pg| *pg != Page::ProfileItems);
        }
        // Instances from before video in recordings and camera ceilings have nothing to choose.
        if !self
            .core
            .shared
            .read(|s| s.instance(&self.key).is_some_and(|i| i.has("video-recordings") || i.has("camera-quality")))
        {
            allowed.retain(|pg| *pg != Page::Recordings);
        }
        // Requests waiting on this server's approval, counted on the menu once the list is read.
        let requests = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.shared.get(&self.server))
                .map_or(0, |l| l.connections.iter().filter(|c| c.home && crate::core::shared::waiting(c)).count())
        });
        if allowed.contains(&Page::Shared) {
            self.load_shared(cx);
        }
        if allowed.contains(&Page::Applications) {
            self.load_applications(false, cx);
        }
        let waiting = self.pages.applications.waiting();
        // Permissions can change while it's open: fall back to a page still yours.
        let page = match self.page.filter(|pg| allowed.contains(pg)).or_else(|| allowed.first().copied()) {
            Some(page) => page,
            None => {
                cx.defer_in(window, |_, _, cx| cx.emit(ServerSettingsEvent::Close));
                return div().into_any_element();
            }
        };
        if self.page != Some(page) {
            self.open(page, cx);
        }

        // The web's layout: the menu takes 15rem and half of what's left past 67rem; the
        // page's column is at most 60rem, less the close button's 4rem and its padding.
        let width = f32::from(window.viewport_size().width);
        let aside = 240.0 + ((width - 1072.0).max(0.0) / 2.0);
        let main = (width - aside).max(320.0);
        let inner = main.min(960.0);
        self.column = inner - 64.0 - 80.0;
        self.wide = width >= 1280.0;

        let badges = move |pg: Page| match pg {
            Page::Shared => requests,
            Page::Applications => waiting,
            _ => 0,
        };
        let menu = self.menu(&server.name, &allowed, page, &badges, &p, window, cx);

        self.bar = None;
        frame::ALARM.with(|a| a.set(self.alarm()));
        let body = match page {
            Page::Overview => self.overview(&server, &p, window, cx),
            Page::Access => self.access_page(&server, &p, window, cx),
            Page::Sso => self.sso_page(&server, &p, window, cx),
            Page::JoinForm => self.join_form_page(&server, &p, window, cx),
            Page::Invites => self.invites_page(&p, window, cx),
            Page::Welcome => self.welcome_page(&server, &p, window, cx),
            Page::Roles => self.roles_page(&p, window, cx),
            Page::Channels => self.channels_page(&p, window, cx),
            Page::Emoji => self.emoji_page(&p, window, cx),
            Page::ProfileItems => {
                let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
                self.profile_items
                    .get_or_insert_with(|| {
                        cx.new(|cx| {
                            crate::ui::profile_items::ProfileItemsView::new(
                                core,
                                key,
                                crate::core::profile_items::Scope::Server(sid),
                                window,
                                cx,
                            )
                        })
                    })
                    .clone()
                    .into_any_element()
            }
            Page::Integrations => {
                let mut both = div().flex().flex_col().gap(px(32.0));
                if access.has(P::ManageServer) {
                    let agents = self.agents_page(&p, window, cx);
                    both = both.child(self.mark("agents", div().child(agents), &p));
                }
                if access.has(P::ManageWebhooks) {
                    let hooks = self.webhooks_page(&p, window, cx);
                    both = both.child(self.mark("webhooks", div().child(hooks), &p));
                }
                both.into_any_element()
            }
            Page::Shared => self.shared_page(&p, window, cx),
            Page::Recordings => self.recordings_page(&server, &p, window, cx),
            Page::Usage => self.usage_page(&p, window, cx),
            Page::Limits => self.limits_page(&p, window, cx),
            Page::Applications => self.applications_page(&server, &p, window, cx),
            Page::Members => self.members_page(&p, window, cx),
            Page::Bans => self.bans_page(&p, window, cx),
            Page::AutoMod => self.automod_page(&p, window, cx),
            Page::AuditLog => self.audit_page(&p, window, cx),
            Page::Ownership => self.ownership_page(&server, &p, window, cx),
            Page::Danger => self.danger_page(&server, &p, window, cx),
        };
        frame::ALARM.with(|a| a.set(None));
        self.held = self.bar.is_some();

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
                // Looked for a moment and it isn't on the page (a role or channel not picked yet).
                None if self.scroll_since.is_some_and(|at| at.elapsed() > Duration::from_millis(1500)) => {
                    self.scroll_to = None
                }
                None => window.request_animation_frame(),
            }
        }

        let about = page.about();
        let header = div()
            .mb(px(24.0))
            .child(
                div()
                    .text_2xl()
                    .line_height(px(32.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .when(page.danger(), |el| el.text_color(p.destructive))
                    .child(tracked(page.label(), TIGHT)),
            )
            .when(!about.is_empty(), |el| {
                el.child(div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(about))
            });
        // The save bar sits under the page, or over the column's foot while the page scrolls
        // (the web's `sticky bottom-0`).
        let scrolls = f32::from(self.scroll.max_offset().y) > 0.5;
        let bar = self.bar.take();
        let (inline_bar, floating_bar) = if scrolls { (None, bar) } else { (bar, None) };
        // The column, and the close button's 4rem beside it (kept clear, as the web's sticky one is).
        let content = div()
            .w(px(main.max(inner)))
            .flex_none()
            .px(px(40.0))
            .pr(px(40.0 + 64.0 + (main.max(inner) - inner)))
            .pt(px(64.0))
            .pb(px(if floating_bar.is_some() { 96.0 } else { 16.0 }))
            .flex()
            .flex_col()
            .child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .child(header)
                    .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(div().mb(px(12.0)).child(e)))
                    .child(body),
                SharedString::from(format!("spage-{page:?}")),
                Duration::ZERO,
                14.0,
            ))
            .when_some(inline_bar, |el, bar| el.child(div().mt(px(24.0)).pb(px(8.0)).child(bar)));

        let (hover_bg, hover_fg, hover_ring) = (p.muted, p.foreground, alpha(p.foreground, 0.4));
        let close = div()
            .id("server-settings-close")
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
                    .id("server-settings-close-ring")
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

        let nickname = self.nickname_dialog(&p, window, cx);
        let behind = crate::ui::backdrop::layers(&crate::ui::theme::backdrop(cx), &p, window, cx);
        let screen = div()
            .id("server-settings")
            .absolute()
            .inset_0()
            // At least the window, so the menu's surface runs down the whole side however short it is.
            .min_w(px(width))
            .min_h(window.viewport_size().height)
            .occlude()
            .flex()
            // The page's surface, out to the window's edge past the close button (the web's
            // screen is `bg-background` and its page draws nothing over it).
            .bg(p.chat_surface)
            .text_color(p.foreground)
            // The web's body line height; Tailwind's text sizes set their own where pages use them.
            .line_height(gpui_kit::relative(1.5))
            .when_some(behind, |el, behind| el.child(behind))
            .child(
                div()
                    .id("server-settings-menu")
                    .flex_none()
                    .w(px(aside))
                    .h(window.viewport_size().height)
                    .flex()
                    .justify_end()
                    // Its own height, not the column's, so a menu taller than the window scrolls.
                    .items_start()
                    .bg(p.side_surface)
                    .border_r_1()
                    .border_color(p.border)
                    .overflow_y_scroll()
                    .child(menu),
            )
            .child(
                div()
                    .id("server-settings-body")
                    .flex_1()
                    .h(window.viewport_size().height)
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .child(close)
            .when_some(floating_bar, |el, bar| {
                el.child(
                    div().absolute().bottom(px(8.0)).left(px(aside + 40.0)).w(px(self.column)).child(motion::rise(
                        div().child(bar),
                        "server-settings-bar",
                        Duration::ZERO,
                        80.0,
                    )),
                )
            })
            .when_some(nickname, |el, d| el.child(d))
            .when_some(crate::ui::cropper::layer(&self.cropper), |el, c| el.child(c));

        // It comes in from a little larger, fading up, and goes the same way.
        use gpui_kit::{Animation, AnimationExt as _};
        let (id, out) = match self.closing {
            Some(at) => (SharedString::from(format!("server-settings-out-{at:?}")), true),
            None => (SharedString::from("server-settings-in"), false),
        };
        let (w, h) = (width, f32::from(window.viewport_size().height));
        screen
            .with_animation(id, Animation::new(frame::OPENING).with_easing(gpui_kit::ease_out_quint()), move |el, t| {
                let k = if out { 1.0 - t } else { t };
                let (dx, dy) = (w * 0.02 * (1.0 - k), h * 0.02 * (1.0 - k));
                el.opacity(k).top(px(-dy)).bottom(px(-dy)).left(px(-dx)).right(px(-dx))
            })
            .into_any_element()
    }
}

pub(crate) fn amber(p: &Palette) -> Hsla {
    hsla(0.11, 0.9, if p.dark { 0.62 } else { 0.42 }, 1.0)
}

pub(crate) fn pill(text: &str, color: Hsla) -> gpui_kit::Div {
    div()
        .flex_none()
        .px(px(7.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .rounded_full()
        .bg(color.opacity(0.15))
        .text_color(color)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(text.to_owned())
}

/// Marks a placeholder's value so `marked` draws it in bold.
pub(crate) fn strong(text: &str) -> String {
    format!("\u{E000}{text}\u{E001}")
}

/// A translated sentence whose `strong` values are drawn in bold.
pub(crate) fn marked(text: &str, p: &Palette) -> gpui_kit::StyledText {
    let parts: Vec<(&str, bool)> =
        text.split(['\u{E000}', '\u{E001}']).enumerate().map(|(k, part)| (part, k % 2 == 1)).collect();
    crate::ui::instance_settings::emphasized(&parts, p)
}

/// The bar of unsaved changes, with Discard and Save (the web's `SaveBar`). The screen puts it
/// under the page or over the column's foot; while someone just tried to leave it shakes and
/// turns red.
pub(crate) fn save_bar<V: 'static>(
    id: &str,
    n: usize,
    saving: bool,
    p: &Palette,
    cx: &mut Context<V>,
    discard: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    save: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    bar_with_error(id, n, saving, None, p, cx, discard, save)
}

/// The same, saying what went wrong with the last save in place of the count.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bar_with_error<V: 'static>(
    id: &str,
    n: usize,
    saving: bool,
    error: Option<&str>,
    p: &Palette,
    cx: &mut Context<V>,
    discard: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    save: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    use crate::ui::settings_controls::{Look, button, shadow_xl};
    let alarm = frame::ALARM.with(|a| a.get());
    let alarmed = alarm.is_some();
    let line: AnyElement = if let Some(e) = error {
        div().text_color(p.destructive).child(crate::ui::instance_home::capitalized(e)).into_any_element()
    } else if alarmed {
        div()
            .font_weight(FontWeight::BOLD)
            .text_color(p.destructive)
            .child(t("settings.controls.careful"))
            .into_any_element()
    } else {
        div()
            .flex()
            .gap(px(4.0))
            .child(div().font_weight(FontWeight::BOLD).child(t("settings.controls.unsaved")))
            .child(
                div()
                    .text_color(p.muted_foreground)
                    .child(t_with("settings.controls.unsavedCount", &[("count", Arg::Num(n as i64))])),
            )
            .into_any_element()
    };
    let bar = div()
        .id(SharedString::from(format!("{id}-card")))
        .occlude()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(12.0))
        .rounded(crate::ui::theme::radius_2xl())
        .border_1()
        .border_color(if alarmed { alpha(p.destructive, 0.7) } else { p.border.into() })
        .bg(if alarmed { mix(p.card, p.destructive, 0.1) } else { alpha(p.card, 0.95) })
        .p(px(12.0))
        .pl(px(16.0))
        .shadow(shadow_xl())
        // The web's `backdrop-blur`: what scrolls under it shows through, blurred.
        .backdrop_blur(px(8.0))
        .child(div().flex_1().min_w_0().text_sm().line_height(px(20.0)).child(line))
        .child(
            button(
                SharedString::from(format!("{id}-discard")),
                t("settings.controls.discard"),
                None,
                Look::Ghost,
                true,
                p,
            )
            .rounded(crate::ui::theme::radius_xl())
            .when(!saving, |el| el.on_click(cx.listener(move |this, _, w, cx| discard(this, w, cx)))),
        )
        .child(
            button(
                SharedString::from(format!("{id}-save")),
                if saving { t("settings.controls.saving") } else { t("settings.controls.save") },
                None,
                Look::Primary,
                true,
                p,
            )
            .rounded(crate::ui::theme::radius_xl())
            .px(px(16.0))
            .font_weight(FontWeight::BOLD)
            .when(saving, |el| el.opacity(0.5))
            .when(!saving, |el| el.on_click(cx.listener(move |this, _, w, cx| save(this, w, cx)))),
        );
    match alarm {
        Some(k) => {
            motion::once(bar, SharedString::from(format!("{id}-shake-{k}")), Duration::from_millis(500), |el, t| {
                let x = [0.0, -10.0, 10.0, -8.0, 8.0, -4.0, 4.0, 0.0];
                let at = t * 7.0;
                let i = (at.floor() as usize).min(6);
                let f = at - i as f32;
                el.relative().left(px(x[i] + (x[i + 1] - x[i]) * f))
            })
        }
        None => bar.into_any_element(),
    }
}

/// A circle that turns while something's on its way.
pub(crate) fn spinner(id: impl Into<SharedString>, size: f32, window: &Window) -> AnyElement {
    motion::ambient(icon("loader-circle").size(px(size)), id.into(), Duration::from_millis(900), window, |el, t| {
        el.rotate(gpui_kit::radians(t * std::f32::consts::TAU))
    })
}

/// Grey bars that pulse while a list loads.
pub(crate) fn shimmer_rows(n: usize, p: &Palette) -> impl IntoElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let base = alpha(p.muted_foreground, 0.1);
    div().flex().flex_col().gap(px(8.0)).children((0..n).map(move |k| {
        div().h(px(58.0)).rounded(corner(16.0)).bg(base).with_animation(
            SharedString::from(format!("shimmer-{k}")),
            Animation::new(Duration::from_millis(1200)).repeat(),
            move |el, t| el.opacity(0.5 + 0.5 * ((t + k as f32 * 0.15) * std::f32::consts::TAU).sin().abs()),
        )
    }))
}

/// The same, keyed by text (a channel's id).
fn text_chips(
    id: &'static str,
    options: &[(String, String)],
    picked: &str,
    p: &Palette,
    cx: &mut Context<ServerSettingsView>,
    pick: fn(&mut ServerSettingsView, String, &mut Context<ServerSettingsView>),
) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .children(options.iter().map(|(value, label)| {
            let on = value == picked;
            let value = value.clone();
            chip(SharedString::from(format!("{id}-{value}")), label, on, p)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !on {
                        pick(this, value.clone(), cx)
                    }
                }))
                .into_any_element()
        }))
        .into_any_element()
}

pub(crate) fn chip(id: SharedString, label: &str, on: bool, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = mix(p.secondary, p.primary, 0.16);
    div()
        .id(id)
        .h(px(32.0))
        .px(px(14.0))
        .rounded_full()
        .flex()
        .items_center()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .map(|el| {
            if on {
                el.bg(p.primary).text_color(p.primary_foreground)
            } else {
                el.bg(p.secondary).text_color(p.foreground).hover(move |s| s.bg(hover))
            }
        })
        .active(|s| s.top(px(1.0)))
        .child(label.to_owned())
}

fn field_label(field: &str) -> String {
    match field {
        "name" => t("serversettings.overview.name"),
        "description" => t("serversettings.nav.description"),
        "icon_url" => t("serversettings.audit.field.icon"),
        "discoverable" => t("serversettings.audit.field.discoverable"),
        "default_notifications" => t("serversettings.nav.defaultNotifications"),
        "system_channel_id" => t("serversettings.nav.joinMessages"),
        "topic" => t("serversettings.channels.topic"),
        "parent_id" => t("serversettings.channels.category"),
        "position" => t("serversettings.audit.field.position"),
        "slowmode_seconds" => t("serversettings.nav.slowmode"),
        "nickname" => t("workspace.moderate.nickname"),
        "role" => t("serversettings.audit.field.role"),
        "timed_out_until" => t("serversettings.audit.field.timedOutUntil"),
        "owner_id" => t("serversettings.shared.owner"),
        "color" => t("serversettings.audit.field.color"),
        "permissions" => t("serversettings.shared.permissions"),
        "hoist" => t("serversettings.audit.field.hoist"),
        "mentionable" => t("serversettings.audit.field.mentionable"),
        "max_uses" => t("serversettings.audit.field.maxUses"),
        "expires_at" => t("serversettings.audit.field.expires"),
        "uses" => t("serversettings.audit.field.uses"),
        "min_account_age_seconds" => t("serversettings.nav.accountAge"),
        "applications" => t("serversettings.nav.applyToJoin"),
        "linked_only" => t("serversettings.nav.linkedOnly"),
        "rules" => t("serversettings.nav.rules"),
        "avatar_url" => t("serversettings.audit.field.picture"),
        "channel_id" => t("serversettings.audit.field.postsIn"),
        "token" => t("serversettings.audit.field.address"),
        "questions" => t("serversettings.audit.field.questions"),
        "enabled" => t("serversettings.audit.field.enabled"),
        "channels" => t("serversettings.nav.channels"),
        "keywords" => t("serversettings.audit.field.keywords"),
        "allowed" => t("serversettings.audit.field.allowed"),
        "mention_limit" => t("serversettings.audit.field.mentionLimit"),
        "actions" => t("serversettings.audit.field.actions"),
        "record_video" => t("serversettings.audit.field.recordVideo"),
        "camera_max_height" => t("serversettings.audit.field.cameraMaxHeight"),
        "camera_max_fps" => t("serversettings.audit.field.cameraMaxFps"),
        other => other.to_owned(),
    }
}

/// A value from the log, in words.
fn value(field: &str, raw: &str, entry: &pb::AuditEntry, people: &People, channels: &[pb::Channel]) -> String {
    let yes_no = |raw: &str| {
        if raw == "true" { t("serversettings.audit.value.yes") } else { t("serversettings.audit.value.no") }
    };
    let at = entry.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
    match field {
        "discoverable" | "enabled" | "hoist" | "mentionable" | "applications" | "linked_only" => yes_no(raw),
        "record_video" => {
            if raw == "true" {
                t("serversettings.audit.value.soundVideo")
            } else {
                t("serversettings.audit.value.soundOnly")
            }
        }
        "camera_max_height" | "camera_max_fps" => {
            let n = raw.parse::<u32>().unwrap_or(0);
            let none = t("serversettings.camera.none");
            if field == "camera_max_height" {
                crate::ui::settings_voice::height_label(n, &none)
            } else {
                crate::ui::settings_voice::fps_label(n, &none)
            }
        }
        "role" => match raw {
            "1" => t("serversettings.audit.rank.member"),
            "2" => t("serversettings.audit.rank.admin"),
            "3" => t("serversettings.shared.owner"),
            "" => t("serversettings.audit.value.none"),
            _ => entry.role_name.clone(),
        },
        "color" if raw.is_empty() => t("serversettings.audit.value.none"),
        "permissions" => {
            let names: Vec<String> = raw
                .split(',')
                .filter_map(|n| n.parse::<i32>().ok())
                .filter_map(|n| P::try_from(n).ok())
                .map(|p| words(&format!("{p:?}")))
                .collect();
            if names.is_empty() { t("serversettings.audit.value.none") } else { names.join(", ") }
        }
        "default_notifications" => {
            match raw.parse::<i32>().ok().and_then(|n| pb::NotificationLevel::try_from(n).ok()) {
                Some(pb::NotificationLevel::Mentions) => t("common.notify.mentions"),
                Some(pb::NotificationLevel::All) => t("common.notify.all"),
                _ => t("serversettings.audit.value.eachOwn"),
            }
        }
        "avatar_url" | "icon_url" => {
            if raw.is_empty() {
                t("serversettings.audit.value.none")
            } else {
                t("serversettings.audit.value.aPicture")
            }
        }
        "token" => t("serversettings.audit.value.replaced"),
        "system_channel_id" | "parent_id" | "channel_id" => {
            if raw.is_empty() {
                return t("serversettings.audit.value.none");
            }
            match channels.iter().find(|c| c.id == raw) {
                Some(c) if field == "parent_id" => c.name.clone(),
                Some(c) => format!("#{}", c.name),
                None => t("serversettings.audit.value.deletedChannel"),
            }
        }
        "slowmode_seconds" => match raw.parse::<i64>().unwrap_or(0) {
            0 => t("serversettings.shared.off"),
            s => duration(s),
        },
        "timed_out_until" => match raw.parse::<i64>() {
            Ok(until) if until > 0 => t_with(
                "serversettings.audit.value.until",
                &[("time", Arg::Str(&stamp(until))), ("duration", Arg::Str(&duration((until - at).max(0) / 1000)))],
            ),
            _ => t("serversettings.audit.value.notTimedOut"),
        },
        "owner_id" => people.get(raw).map(user_name).unwrap_or_else(|| t("common.someone")),
        "max_uses" if raw == "0" => t("settings.controls.noLimit"),
        "expires_at" => match raw.parse::<i64>() {
            Ok(ms) if ms > 0 => stamp(ms),
            _ => t("serversettings.shared.never"),
        },
        "min_account_age_seconds" => match raw.parse::<i64>().unwrap_or(0) {
            0 => t("serversettings.access.age.any"),
            s => duration(s),
        },
        _ if raw.is_empty() => t("serversettings.audit.value.nothing"),
        _ => raw.to_owned(),
    }
}

/// "ManageServer" → "Manage server".
fn words(name: &str) -> String {
    let mut out = String::new();
    for (k, c) in name.chars().enumerate() {
        if c.is_uppercase() && k > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Text safe to put in Markdown: names can't make links or emphasis.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\`*_{}[]()#+-.!<>|~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// What an entry says happened, in Markdown, names in bold.
pub fn sentence(entry: &pb::AuditEntry, people: &People, channels: &[pb::Channel]) -> String {
    let who =
        |id: &str| format!("**{}**", plain(&people.get(id).map(user_name).unwrap_or_else(|| t("common.someone"))));
    let actor = who(&entry.actor_id);
    let target = who(&entry.target_id);
    let change = |field: &str| entry.changes.iter().find(|c| c.field == field);
    let shared_server = || {
        let name = change("server").map(|c| c.after.clone()).unwrap_or_else(|| t("serversettings.audit.anotherServer"));
        format!("**{}**", plain(&name))
    };
    let at = entry.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
    let named_channel = |name: &str| format!("**#{}**", plain(name));
    let channel = match channels.iter().find(|c| c.id == entry.target_id) {
        Some(c) if c.r#type == pb::ChannelType::Category as i32 => format!("**{}**", plain(&c.name)),
        Some(c) => named_channel(&c.name),
        None => named_channel(&entry.channel_name),
    };
    let a_role = t("serversettings.audit.aRole");
    let role = format!("**{}**", plain(if entry.role_name.is_empty() { &a_role } else { &entry.role_name }));
    let only = |field: &str| entry.changes.len() == 1 && change(field).is_some();
    let name_of = |side_after: bool| {
        change("name").map(|c| plain(if side_after { &c.after } else { &c.before })).unwrap_or_default()
    };
    let (before, after) = (format!("**{}**", name_of(false)), format!("**{}**", name_of(true)));
    let (emoji_before, emoji_after) = (format!("**:{}:**", name_of(false)), format!("**:{}:**", name_of(true)));
    // Where it happened, by the name the log kept.
    let place = named_channel(&entry.channel_name);
    let automod = format!("**{}**", t("serversettings.nav.automod"));
    let span = |until: i64| duration(((until - at) / 1000).max(1));
    match entry.action() {
        A::ServerUpdate => {
            if only("record_video") {
                if change("record_video").is_some_and(|c| c.after == "true") {
                    t_with("serversettings.audit.s.recordVideoOn", &[("actor", Arg::Str(&actor))])
                } else {
                    t_with("serversettings.audit.s.recordSoundOnly", &[("actor", Arg::Str(&actor))])
                }
            } else if only("applications") {
                if change("applications").is_some_and(|c| c.after == "true") {
                    t_with("serversettings.audit.s.applyOn", &[("actor", Arg::Str(&actor))])
                } else {
                    t_with("serversettings.audit.s.applyOff", &[("actor", Arg::Str(&actor))])
                }
            } else if only("discoverable") {
                if change("discoverable").is_some_and(|c| c.after == "true") {
                    t_with("serversettings.audit.s.listed", &[("actor", Arg::Str(&actor))])
                } else {
                    t_with("serversettings.audit.s.inviteOnly", &[("actor", Arg::Str(&actor))])
                }
            } else if only("name") {
                t_with("desktop.server.audit.renamedServer", &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&after))])
            } else {
                t_with("serversettings.audit.s.serverUpdate", &[("actor", Arg::Str(&actor))])
            }
        }
        A::ChannelCreate => t_with(
            "serversettings.audit.s.channelCreate",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&channel))],
        ),
        A::ChannelUpdate => match change("slowmode_seconds").filter(|_| entry.changes.len() == 1) {
            Some(c) if c.after == "0" => t_with(
                "serversettings.audit.s.slowOff",
                &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&channel))],
            ),
            Some(c) => t_with(
                "serversettings.audit.s.slowSet",
                &[
                    ("actor", Arg::Str(&actor)),
                    ("channel", Arg::Str(&channel)),
                    ("duration", Arg::Str(&duration(c.after.parse().unwrap_or(0)))),
                ],
            ),
            None => t_with(
                "serversettings.audit.s.channelUpdate",
                &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&channel))],
            ),
        },
        A::ChannelDelete => t_with(
            "serversettings.audit.s.channelDelete",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place))],
        ),
        A::ChannelsReorder => t_with("serversettings.audit.s.channelsReorder", &[("actor", Arg::Str(&actor))]),
        A::MemberUpdate => {
            t_with("serversettings.audit.s.nickname", &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))])
        }
        A::MemberRolesUpdate => match change("role") {
            Some(c) if !c.after.is_empty() => t_with(
                "serversettings.audit.s.roleGive",
                &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("role", Arg::Str(&role))],
            ),
            _ => t_with(
                "serversettings.audit.s.roleTake",
                &[("actor", Arg::Str(&actor)), ("role", Arg::Str(&role)), ("target", Arg::Str(&target))],
            ),
        },
        A::RoleCreate => {
            t_with("serversettings.audit.s.roleCreate", &[("actor", Arg::Str(&actor)), ("role", Arg::Str(&role))])
        }
        A::RoleUpdate => {
            if only("name") {
                t_with(
                    "serversettings.audit.s.renamed",
                    &[("actor", Arg::Str(&actor)), ("before", Arg::Str(&before)), ("after", Arg::Str(&after))],
                )
            } else if only("permissions") {
                t_with(
                    "serversettings.audit.s.rolePermissions",
                    &[("actor", Arg::Str(&actor)), ("role", Arg::Str(&role))],
                )
            } else {
                t_with("serversettings.audit.s.roleUpdate", &[("actor", Arg::Str(&actor)), ("role", Arg::Str(&role))])
            }
        }
        A::RoleDelete => {
            t_with("serversettings.audit.s.roleDelete", &[("actor", Arg::Str(&actor)), ("role", Arg::Str(&role))])
        }
        A::RolesReorder => t_with("serversettings.audit.s.rolesReorder", &[("actor", Arg::Str(&actor))]),
        A::ChannelPermissionsUpdate => t_with(
            "serversettings.audit.s.channelPermissions",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&channel))],
        ),
        A::MemberTimeOut => match change("timed_out_until").and_then(|c| c.after.parse::<i64>().ok()) {
            Some(until) if until > 0 => t_with(
                "serversettings.audit.s.timeOut",
                &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("duration", Arg::Str(&span(until)))],
            ),
            _ => t_with(
                "serversettings.audit.s.timeOutEnd",
                &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))],
            ),
        },
        A::MemberKick => {
            t_with("serversettings.audit.s.kick", &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))])
        }
        A::MemberBan => {
            let deleted: i64 = change("deleted_messages").and_then(|c| c.after.parse().ok()).unwrap_or(0);
            match deleted {
                0 => {
                    t_with("serversettings.audit.s.ban", &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))])
                }
                n => t_with(
                    "serversettings.audit.s.banDeleted",
                    &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("count", Arg::Num(n))],
                ),
            }
        }
        A::MemberUnban => {
            t_with("serversettings.audit.s.unban", &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))])
        }
        A::MessageDelete => t_with(
            "serversettings.audit.s.messageDelete",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::MessagePin => t_with(
            "serversettings.audit.s.messagePin",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::MessageUnpin => t_with(
            "serversettings.audit.s.messageUnpin",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::OwnershipTransfer => {
            t_with("serversettings.audit.s.ownership", &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))])
        }
        A::LiveTileEnd => t_with(
            "serversettings.audit.s.liveTileEnd",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::InviteCreate if !entry.channel_name.is_empty() => t_with(
            "serversettings.audit.s.inviteCreateIn",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place))],
        ),
        A::InviteCreate => t_with("serversettings.audit.s.inviteCreate", &[("actor", Arg::Str(&actor))]),
        A::InviteDelete if !entry.channel_name.is_empty() => t_with(
            "serversettings.audit.s.inviteDeleteIn",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place))],
        ),
        A::InviteDelete => t_with("serversettings.audit.s.inviteDelete", &[("actor", Arg::Str(&actor))]),
        A::ApplicationApprove => t_with(
            "serversettings.audit.s.applicationApprove",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))],
        ),
        A::ApplicationReject => t_with(
            "serversettings.audit.s.applicationReject",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))],
        ),
        A::JoinFormUpdate => match (change("rules").is_some(), change("questions").is_some()) {
            (true, false) => t_with("serversettings.audit.s.rulesChanged", &[("actor", Arg::Str(&actor))]),
            (false, true) => t_with("serversettings.audit.s.questionsChanged", &[("actor", Arg::Str(&actor))]),
            _ => t_with("serversettings.audit.s.joinFormChanged", &[("actor", Arg::Str(&actor))]),
        },
        A::WelcomeScreenUpdate => match change("enabled").filter(|_| entry.changes.len() == 1) {
            Some(c) if c.after == "true" => t_with("serversettings.audit.s.welcomeOn", &[("actor", Arg::Str(&actor))]),
            Some(_) => t_with("serversettings.audit.s.welcomeOff", &[("actor", Arg::Str(&actor))]),
            None => t_with("serversettings.audit.s.welcomeChanged", &[("actor", Arg::Str(&actor))]),
        },
        A::OnboardingUpdate => match change("enabled").filter(|_| entry.changes.len() == 1) {
            Some(c) if c.after == "true" => {
                t_with("serversettings.audit.s.onboardingOn", &[("actor", Arg::Str(&actor))])
            }
            Some(_) => t_with("serversettings.audit.s.onboardingOff", &[("actor", Arg::Str(&actor))]),
            None => t_with("desktop.server.audit.onboardingChanged", &[("actor", Arg::Str(&actor))]),
        },
        A::AutoModRuleCreate => {
            t_with("serversettings.audit.s.automodCreate", &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&after))])
        }
        A::AutoModRuleUpdate => {
            t_with("serversettings.audit.s.automodChanged", &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&after))])
        }
        A::AutoModRuleDelete => {
            t_with("serversettings.audit.s.automodDelete", &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&before))])
        }
        A::AutoModTimeOut => {
            let until = change("timed_out_until").and_then(|c| c.after.parse::<i64>().ok());
            match (until, entry.channel_name.is_empty()) {
                (Some(until), false) => t_with(
                    "serversettings.audit.s.automodTimeOutForIn",
                    &[
                        ("automod", Arg::Str(&automod)),
                        ("target", Arg::Str(&target)),
                        ("duration", Arg::Str(&span(until))),
                        ("channel", Arg::Str(&place)),
                    ],
                ),
                (Some(until), true) => t_with(
                    "serversettings.audit.s.automodTimeOutFor",
                    &[
                        ("automod", Arg::Str(&automod)),
                        ("target", Arg::Str(&target)),
                        ("duration", Arg::Str(&span(until))),
                    ],
                ),
                (None, false) => t_with(
                    "serversettings.audit.s.automodTimeOutIn",
                    &[("automod", Arg::Str(&automod)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
                ),
                (None, true) => t_with(
                    "serversettings.audit.s.automodTimeOut",
                    &[("automod", Arg::Str(&automod)), ("target", Arg::Str(&target))],
                ),
            }
        }
        A::AutoModMessageDelete => {
            if entry.channel_name.is_empty() {
                t_with(
                    "serversettings.audit.s.automodTakedown",
                    &[("automod", Arg::Str(&automod)), ("target", Arg::Str(&target))],
                )
            } else {
                t_with(
                    "serversettings.audit.s.automodTakedownIn",
                    &[("automod", Arg::Str(&automod)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
                )
            }
        }
        A::EmojiCreate => t_with(
            "serversettings.audit.s.emojiCreate",
            &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&emoji_after))],
        ),
        A::EmojiUpdate => t_with(
            "serversettings.audit.s.renamed",
            &[("actor", Arg::Str(&actor)), ("before", Arg::Str(&emoji_before)), ("after", Arg::Str(&emoji_after))],
        ),
        A::EmojiDelete => t_with(
            "serversettings.audit.s.emojiDelete",
            &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&emoji_before))],
        ),
        A::ProfileItemCreate => t_with(
            "serversettings.audit.s.profileItemCreate",
            &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&after))],
        ),
        A::ProfileItemUpdate if only("name") => t_with(
            "serversettings.audit.s.renamed",
            &[("actor", Arg::Str(&actor)), ("before", Arg::Str(&before)), ("after", Arg::Str(&after))],
        ),
        A::ProfileItemUpdate => {
            let name = if change("name").is_some() {
                after.clone()
            } else {
                format!("**{}**", t("serversettings.audit.aProfileItem"))
            };
            t_with(
                "serversettings.audit.s.profileItemUpdate",
                &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&name))],
            )
        }
        A::ProfileItemDelete => t_with(
            "serversettings.audit.s.profileItemDelete",
            &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&before))],
        ),
        A::WebhookCreate => t_with(
            "serversettings.audit.s.webhookCreate",
            &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&after)), ("channel", Arg::Str(&place))],
        ),
        A::WebhookUpdate => {
            t_with("desktop.server.audit.webhookUpdate", &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&after))])
        }
        A::WebhookDelete => {
            t_with("serversettings.audit.s.webhookDelete", &[("actor", Arg::Str(&actor)), ("name", Arg::Str(&before))])
        }
        A::AgentAdd => {
            t_with("serversettings.audit.s.agentAdd", &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target))])
        }
        A::ShareCodeCreate => t_with(
            "serversettings.audit.s.shareCodeCreate",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place))],
        ),
        A::ShareCodeDelete => t_with(
            "serversettings.audit.s.shareCodeDelete",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place))],
        ),
        A::SharedChannelRequest => t_with(
            "desktop.server.audit.sharedRequest",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place)), ("server", Arg::Str(&shared_server()))],
        ),
        A::SharedChannelApprove => t_with(
            "serversettings.audit.s.sharedApprove",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place)), ("server", Arg::Str(&shared_server()))],
        ),
        A::SharedChannelUpdate => {
            t_with("desktop.server.audit.sharedUpdate", &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place))])
        }
        A::SharedChannelDisconnect => t_with(
            "serversettings.audit.s.sharedDisconnectWith",
            &[("actor", Arg::Str(&actor)), ("channel", Arg::Str(&place)), ("server", Arg::Str(&shared_server()))],
        ),
        A::SharedChannelBlock => t_with(
            "serversettings.audit.s.sharedBlock",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::SharedChannelUnblock => t_with(
            "serversettings.audit.s.sharedUnblock",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::ThreadLock => t_with(
            "serversettings.audit.s.threadLock",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::ThreadUnlock => t_with(
            "serversettings.audit.s.threadUnlock",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::ThreadDelete => t_with(
            "serversettings.audit.s.threadDelete",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::PollEnd => t_with(
            "serversettings.audit.s.pollEndIn",
            &[("actor", Arg::Str(&actor)), ("target", Arg::Str(&target)), ("channel", Arg::Str(&place))],
        ),
        A::Unspecified => t_with("serversettings.audit.s.unknown", &[("actor", Arg::Str(&actor))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, name: &str) -> pb::User {
        pb::User { id: id.into(), username: name.into(), display_name: name.into(), ..Default::default() }
    }

    #[test]
    fn entries_read_like_sentences() {
        let people: People =
            [user("a", "Mika"), user("b", "Hana_*x*")].into_iter().map(|u| (u.id.clone(), u)).collect();
        let mut entry = pb::AuditEntry {
            actor_id: "a".into(),
            target_id: "b".into(),
            action: A::MemberBan as i32,
            changes: vec![pb::AuditChange {
                field: "deleted_messages".into(),
                before: String::new(),
                after: "3".into(),
            }],
            ..Default::default()
        };
        assert_eq!(sentence(&entry, &people, &[]), "**Mika** banned **Hana\\_\\*x\\*** and deleted 3 messages");
        entry.action = A::MemberTimeOut as i32;
        entry.created_at = Some(prost_types::Timestamp { seconds: 100, nanos: 0 });
        entry.changes =
            vec![pb::AuditChange { field: "timed_out_until".into(), before: String::new(), after: "700000".into() }];
        assert_eq!(sentence(&entry, &people, &[]), "**Mika** timed out **Hana\\_\\*x\\*** for 10 minutes");
        entry.changes[0].after = String::new();
        assert!(sentence(&entry, &people, &[]).ends_with("'s time-out"));
    }

    #[test]
    fn pages_follow_permissions() {
        use crate::core::permissions::Access;
        let nobody = Access::default();
        assert!(pages(&nobody, false).is_empty());
        let channels = std::iter::once(("general".to_owned(), u32::MAX)).collect();
        let owner = Access { owner: true, server: u32::MAX, channels, ..Access::default() };
        assert_eq!(words("ManageServer"), "Manage server");
        let mut all = ALL.to_vec();
        all.retain(|p| *p != Page::Limits);
        assert_eq!(pages(&owner, false), all);
        assert_eq!(pages(&owner, true), ALL.to_vec());
        let hooks = Access { server: crate::core::permissions::bit(P::ManageWebhooks), ..Access::default() };
        assert_eq!(pages(&hooks, false), vec![Page::Integrations]);
        // An instance admin who isn't in charge here sees the usage, the caps and the way to delete it.
        assert_eq!(pages(&nobody, true), vec![Page::Usage, Page::Limits, Page::Danger]);
        // Managing one channel opens the Channels page.
        let one = std::iter::once(("general".to_owned(), crate::core::permissions::bit(P::ManageRoles))).collect();
        let keeper = Access { channels: one, ..Access::default() };
        assert_eq!(pages(&keeper, false), vec![Page::Channels]);
    }

    #[test]
    fn search_finds_pages_and_settings() {
        let found = search(&ALL, "slow").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, Page::Channels);
        assert_eq!(found[0].1[0].0, "slowmode");
        assert!(search(&ALL, "  ").is_none());
        assert!(search(&ALL, "zzzz").unwrap().is_empty());
    }
}
