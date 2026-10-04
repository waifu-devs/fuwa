//! A server's settings, full screen with a side menu like the app's own:
//! its name, picture and words, the welcome screen, invites, roles, emoji,
//! agents and webhooks, members, bans, AutoMod and the audit log.
//! Each page shows only to people whose permissions open it, as in the web
//! app's `ServerSettingsDialog.tsx`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    hsla, px,
};

use crate::core::Core;
use crate::core::api::Problem;
use crate::core::dms::now_ms;
use crate::core::moderation::{Action, timed_out_until};
use crate::core::server_admin::{People, ServerPatch};
use crate::core::store::user_name;
use crate::pb::{self, AuditAction as A, Permission as P};
use crate::ui::moderate::{duration, stamp};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::widgets::{
    app_badge, avatar, danger_button, error_line, icon, icon_button, icon_button_in, is_agent, labeled, pal,
    primary_button, server_icon, soft_button,
};

mod agents;
mod automod;
mod channels;
mod emoji;
pub(crate) mod roles;
pub(crate) use roles::switch;
mod shared;
mod webhooks;
mod welcome;

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
    /// Say something for a moment, over the settings.
    Toast {
        icon: &'static str,
        title: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Overview,
    Welcome,
    Invites,
    Roles,
    Channels,
    Emoji,
    Integrations,
    Shared,
    Members,
    Bans,
    AutoMod,
    AuditLog,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Welcome => "Welcome screen",
            Page::Invites => "Invites",
            Page::Roles => "Roles",
            Page::Channels => "Channels",
            Page::Emoji => "Emoji",
            Page::Integrations => "Integrations",
            Page::Shared => "Shared channels",
            Page::Members => "Members",
            Page::Bans => "Bans",
            Page::AutoMod => "AutoMod",
            Page::AuditLog => "Audit log",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Page::Overview => "settings",
            Page::Welcome => "party-popper",
            Page::Invites => "link",
            Page::Roles => "shield",
            Page::Channels => "hash",
            Page::Emoji => "face-slightly-smiling-plus",
            Page::Integrations => "webhook",
            Page::Shared => "link-2",
            Page::Members => "users",
            Page::Bans => "gavel",
            Page::AutoMod => "bot",
            Page::AuditLog => "scroll-text",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Page::Overview => "Its name, picture and a few words, and how it notifies people by default.",
            Page::Invites => "The links that let people in. Revoke one and it stops working at once.",
            Page::Roles => "Who can do what. Members take the color of their highest role.",
            Page::Channels => "Order, categories, topics, slow mode, and who can see and use each.",
            Page::Emoji => "The server's own emoji. Everyone here can use them as :name:.",
            Page::Welcome => "What new members see first: a few words and channels to start in.",
            Page::Integrations => {
                "Agents, accounts programs drive, and webhooks, addresses other apps post messages to."
            }
            Page::Shared => {
                "Channels shown in another server, or from one. Messages stay with the server the channel comes from."
            }
            Page::Members => "Everyone here. Time out, kick or ban the people you rank above.",
            Page::Bans => "Who's kept out, and why.",
            Page::AutoMod => {
                "Rules that catch messages as they're sent: blocked words, mention spam, links and a smart filter."
            }
            Page::AuditLog => "Every change people made here with their permissions.",
        }
    }
}

/// The settings group, then the moderation group, as on the web.
const SETTINGS: [Page; 8] = [
    Page::Overview,
    Page::Welcome,
    Page::Invites,
    Page::Roles,
    Page::Channels,
    Page::Emoji,
    Page::Integrations,
    Page::Shared,
];
const MODERATION: [Page; 4] = [Page::Members, Page::Bans, Page::AutoMod, Page::AuditLog];

/// The pages someone with this access may open.
fn pages(access: &crate::core::permissions::Access) -> Vec<Page> {
    let invites = access.has(P::ManageServer)
        || access.has(P::CreateInvite)
        || access.channels.keys().any(|c| access.has_in(c, P::CreateInvite));
    let members = [P::KickMembers, P::BanMembers, P::TimeOutMembers, P::ManageNicknames, P::ManageRoles]
        .into_iter()
        .any(|p| access.has(p));
    let mut out = Vec::new();
    if access.has(P::ManageServer) {
        out.push(Page::Overview);
        out.push(Page::Welcome);
    }
    if invites {
        out.push(Page::Invites);
    }
    if access.has(P::ManageRoles) {
        out.push(Page::Roles);
    }
    if access.channels.keys().any(|c| access.has_in(c, P::ManageChannels) || access.has_in(c, P::ManageRoles)) {
        out.push(Page::Channels);
    }
    if access.has(P::ManageEmoji) {
        out.push(Page::Emoji);
    }
    if access.has(P::ManageWebhooks) || access.has(P::ManageServer) {
        out.push(Page::Integrations);
    }
    if access.has(P::ManageServer) {
        out.push(Page::Shared);
    }
    if members {
        out.push(Page::Members);
    }
    if access.has(P::BanMembers) {
        out.push(Page::Bans);
    }
    if access.has(P::ManageServer) {
        out.push(Page::AutoMod);
    }
    if access.has(P::ViewAuditLog) {
        out.push(Page::AuditLog);
    }
    out
}

/// Whether someone with this access gets server settings at all.
pub fn can_open(access: &crate::core::permissions::Access) -> bool {
    !pages(access).is_empty()
}

/// What the audit log can be narrowed to, as chips.
const FILTERS: [(A, &str); 8] = [
    (A::Unspecified, "Anything"),
    (A::MemberTimeOut, "Time-outs"),
    (A::MemberKick, "Kicks"),
    (A::MemberBan, "Bans"),
    (A::MemberUnban, "Unbans"),
    (A::MessageDelete, "Deleted messages"),
    (A::ServerUpdate, "Server settings"),
    (A::InviteCreate, "New invites"),
];

pub struct ServerSettingsView {
    core: Arc<Core>,
    pub key: String,
    pub server: String,
    page: Option<Page>,
    name: Entity<InputState>,
    description: Entity<TextareaState>,
    member_query: Entity<InputState>,
    /// Which server the fields were filled from, so they fill once.
    filled: bool,
    busy: bool,
    uploading: bool,
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
    shared: shared::Shared,
    /// A floating bar of changes not saved yet, drawn over the page's foot.
    bar: Option<AnyElement>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ServerSettingsEvent> for ServerSettingsView {}

impl ServerSettingsView {
    pub fn new(core: Arc<Core>, key: String, server: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("My cozy server"));
        let description =
            cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).placeholder("What's this server about?"));
        let member_query = cx.new(|cx| InputState::new(window, cx).placeholder("Find someone"));
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
        ];
        subscriptions.extend(role_subscriptions);
        subscriptions.extend(emoji_subscriptions);
        subscriptions.extend(hook_subscriptions);
        subscriptions.extend(agent_subscriptions);
        subscriptions.extend(automod_subscriptions);
        subscriptions.extend(welcome_subscriptions);
        subscriptions.extend(channel_subscriptions);
        subscriptions.extend(shared_subscriptions);
        Self {
            core,
            key,
            server,
            page: None,
            name,
            description,
            member_query,
            filled: false,
            busy: false,
            uploading: false,
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
            channels,
            shared,
            bar: None,
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
        self.run(cx, async move { core.audit_log(&key, &sid, &before, "", action).await }, move |this, result, cx| {
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
        });
    }

    fn save(&mut self, patch: ServerPatch, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.update_server(&key, &sid, patch).await }, |this, result, cx| {
            this.busy = false;
            match result {
                Ok(_) => this.flash_saved(cx),
                Err(err) => this.error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
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

    /// Asks the system for a picture and makes it the server's icon.
    fn pick_icon(&mut self, cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a picture".into()),
        });
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(kind) = crate::core::account::picture_type(&name) else {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some("That isn't a picture fuwa can use (PNG, JPEG, GIF or WebP).".into());
                    cx.notify();
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.uploading = true;
                this.error = None;
                cx.notify();
            });
            let rx = core.spawn({
                let core = core.clone();
                async move {
                    let bytes = tokio::fs::read(&path).await.map_err(|err| {
                        Problem::new(tonic::Code::NotFound, format!("Couldn't read that file: {err}"))
                    })?;
                    let url = core.upload_picture(&key, pb::MediaPurpose::ServerIcon, kind, bytes).await?;
                    core.update_server(&key, &sid, ServerPatch { icon_url: Some(url), ..ServerPatch::default() }).await
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.uploading = false;
                match result {
                    Ok(Ok(_)) => this.flash_saved(cx),
                    Ok(Err(err)) => this.error = Some(err.message),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn overview(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.filled {
            self.filled = true;
            let (name, about) = (server.name.clone(), server.description.clone());
            self.name.update(cx, |s, cx| s.set_value(name, window, cx));
            self.description.update(cx, |s, cx| s.set_value(about, window, cx));
        }
        let name = self.name.read(cx).value().trim().to_owned();
        let about = self.description.read(cx).value().trim().to_owned();
        let dirty = name != server.name || about != server.description;
        let uploading = self.uploading;

        let picture = div()
            .id("server-icon")
            .relative()
            .size(px(96.0))
            .flex_none()
            .cursor_pointer()
            .group("icon")
            .child(server_icon(server, 96.0, 32.0, p))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(32.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .bg(alpha(p.rail, 0.6))
                    .text_color(gpui_kit::white())
                    .text_xs()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .opacity(if uploading { 1.0 } else { 0.0 })
                    .group_hover("icon", |s| s.opacity(1.0))
                    .child(icon(if uploading { "loader-circle" } else { "camera" }).size(px(20.0)))
                    .child(if uploading { "Uploading…" } else { "Change" }),
            )
            .on_click(cx.listener(|this, _, _, cx| this.pick_icon(cx)));

        let level = server.default_notifications();
        let channels: Vec<pb::Channel> = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.channels.get(&self.server))
                .into_iter()
                .flatten()
                .filter(|c| c.r#type == pb::ChannelType::Text as i32)
                .cloned()
                .collect()
        });
        let mut joins: Vec<(String, String)> = vec![(String::new(), "Off".into())];
        joins.extend(channels.iter().map(|c| (c.id.clone(), format!("#{}", c.name))));

        div()
            .flex()
            .flex_col()
            .gap(px(22.0))
            .child(
                div().flex().gap(px(20.0)).items_start().child(picture).child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(labeled("Server name", Input::new(&self.name).large(), p))
                        .child(labeled("Description", Textarea::new(&self.description), p)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(10.0))
                    .when_some(self.saved.filter(|t| t.elapsed() < Duration::from_secs(3)), |el, _| {
                        el.child(motion::rise(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.success)
                                .child(icon("check").size(px(16.0)))
                                .child("Saved"),
                            "saved",
                            Duration::ZERO,
                            6.0,
                        ))
                    })
                    .child(
                        primary_button("server-save", if self.busy { "Saving…" } else { "Save changes" }, p)
                            .when(!dirty || name.is_empty(), |el| el.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !dirty || this.busy {
                                    return;
                                }
                                let name = this.name.read(cx).value().trim().to_owned();
                                if name.is_empty() {
                                    this.error = Some("Give it a name.".into());
                                    cx.notify();
                                    return;
                                }
                                let about = this.description.read(cx).value().trim().to_owned();
                                this.save(
                                    ServerPatch {
                                        name: Some(name),
                                        description: Some(about),
                                        ..ServerPatch::default()
                                    },
                                    cx,
                                );
                            })),
                    ),
            )
            .child(labeled(
                "Default notifications",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(chips(
                        "notify",
                        &[
                            (pb::NotificationLevel::All as i64, "All messages".to_owned()),
                            (pb::NotificationLevel::Mentions as i64, "Only @mentions".to_owned()),
                        ],
                        level as i64,
                        p,
                        cx,
                        |this, v, cx| {
                            let level = pb::NotificationLevel::try_from(v as i32).unwrap_or_default();
                            this.save(ServerPatch { default_notifications: Some(level), ..ServerPatch::default() }, cx);
                        },
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child("For people who haven't picked their own. Big servers usually pick only @mentions."),
                    ),
                p,
            ))
            .child(labeled(
                "Join messages",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(text_chips("joins", &joins, &server.system_channel_id, p, cx, |this, id, cx| {
                        this.save(ServerPatch { system_channel_id: Some(id), ..ServerPatch::default() }, cx);
                    }))
                    .child(
                        div().text_xs().text_color(p.muted_foreground).child("Where fuwa says hi when someone joins."),
                    ),
                p,
            ))
            .into_any_element()
    }

    fn invites_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let base = self.core.api(&self.key).map(|a| a.url.trim_end_matches('/').to_owned()).unwrap_or_default();
        let (me, manage, channels) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (
                i.and_then(|i| i.me.as_ref().map(|m| m.id.clone())).unwrap_or_default(),
                i.is_some_and(|i| i.access(&self.server).has(P::ManageServer)),
                i.and_then(|i| i.channels.get(&self.server).cloned()).unwrap_or_default(),
            )
        });
        let make = primary_button("invite-make", "Make an invite", p).child(icon("plus").size(px(16.0))).on_click(
            cx.listener(|this, _, _, cx| {
                let (core, key, sid) = (this.core.clone(), this.key.clone(), this.server.clone());
                this.run(cx, async move { core.create_invite(&key, &sid).await }, |this, result, cx| {
                    match result {
                        Ok(_) => this.load_invites(cx),
                        Err(err) => this.error = Some(err.message),
                    }
                    cx.notify();
                });
            }),
        );
        let mut list = div().flex().flex_col().gap(px(8.0));
        match &self.invites {
            None => list = list.child(shimmer_rows(3, p)),
            Some((invites, _)) if invites.is_empty() => {
                list = list.child(empty("link", "No invites yet", "Make one and send it to someone.", p))
            }
            Some((invites, people)) => {
                let now = now_ms();
                for (n, invite) in invites.iter().enumerate() {
                    let link = format!("{base}/invite/{}", invite.code);
                    let inviter = people.get(&invite.inviter_id);
                    let channel = channels.iter().find(|c| c.id == invite.channel_id).map(|c| format!("#{}", c.name));
                    let uses = if invite.max_uses > 0 {
                        format!("{} of {} used", invite.uses, invite.max_uses)
                    } else {
                        format!("{} {}", invite.uses, if invite.uses == 1 { "use" } else { "uses" })
                    };
                    let expires = match invite.expires_at.as_ref().map(|t| t.seconds * 1000) {
                        None => "Never expires".to_owned(),
                        Some(ms) if ms <= now => "Expired".to_owned(),
                        Some(ms) => format!("Expires {}", stamp(ms)),
                    };
                    let copied = self
                        .copied
                        .as_ref()
                        .is_some_and(|(c, at)| *c == invite.code && at.elapsed() < Duration::from_secs(2));
                    let can_revoke = manage || invite.inviter_id == me;
                    let code = invite.code.clone();
                    list = list.child(motion::rise(
                        row(p)
                            .child(
                                div()
                                    .size(px(40.0))
                                    .flex_none()
                                    .rounded(corner(12.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(alpha(p.primary, 0.12))
                                    .text_color(p.primary)
                                    .child(icon("link").size(px(18.0))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(link.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .text_xs()
                                            .text_color(p.muted_foreground)
                                            .child(avatar(inviter, 16.0, p))
                                            .child(inviter.map(user_name).unwrap_or_else(|| "Someone".into()))
                                            .when_some(channel, |el, c| el.child("·").child(c))
                                            .child("·")
                                            .child(uses)
                                            .child("·")
                                            .child(expires),
                                    ),
                            )
                            .child(
                                icon_button(
                                    SharedString::from(format!("copy-{code}")),
                                    if copied { "check" } else { "copy" },
                                    p,
                                )
                                .when(copied, |el| el.text_color(p.success))
                                .on_click(cx.listener({
                                    let (code, link) = (code.clone(), link.clone());
                                    move |this, _, _, cx| {
                                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link.clone()));
                                        this.copied = Some((code.clone(), Instant::now()));
                                        cx.notify();
                                    }
                                })),
                            )
                            .when(can_revoke, |el| {
                                el.child(
                                    icon_button_in(
                                        SharedString::from(format!("revoke-{code}")),
                                        "link-2-off",
                                        p,
                                        p.destructive,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            let (core, key, sid, code) = (
                                                this.core.clone(),
                                                this.key.clone(),
                                                this.server.clone(),
                                                code.clone(),
                                            );
                                            let gone = code.clone();
                                            this.run(
                                                cx,
                                                async move { core.delete_invite(&key, &sid, &code).await },
                                                move |this, result, cx| {
                                                    match result {
                                                        Ok(()) => {
                                                            if let Some((list, _)) = &mut this.invites {
                                                                list.retain(|i| i.code != gone);
                                                            }
                                                        }
                                                        Err(err) => this.error = Some(err.message),
                                                    }
                                                    cx.notify();
                                                },
                                            );
                                        },
                                    )),
                                )
                            }),
                        SharedString::from(format!("invite-in-{}", invite.code)),
                        Duration::from_millis(30 * n.min(12) as u64),
                        8.0,
                    ));
                }
            }
        }
        div().flex().flex_col().gap(px(16.0)).child(div().flex().child(make)).child(list).into_any_element()
    }

    fn members_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.member_query.read(cx).value().trim().to_lowercase();
        let now = now_ms();
        let amber = amber(p);
        let rows: Vec<MemberRow> = self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return Vec::new() };
            let owner = i.server(&self.server).map(|s| s.owner_id.clone()).unwrap_or_default();
            let roles = i.roles.get(&self.server).cloned().unwrap_or_default();
            i.members
                .get(&self.server)
                .into_iter()
                .flatten()
                .filter(|m| {
                    query.is_empty()
                        || m.user.as_ref().is_some_and(|u| {
                            u.username.to_lowercase().contains(&query)
                                || user_name(u).to_lowercase().contains(&query)
                                || m.nickname.to_lowercase().contains(&query)
                        })
                })
                .map(|m| {
                    let uid = m.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                    let held: Vec<(String, Option<u32>)> = roles
                        .iter()
                        .filter(|r| m.role_ids.contains(&r.id))
                        .map(|r| (r.name.clone(), r.color.map(|c| c as u32)))
                        .collect();
                    (m.clone(), i.can_moderate(&self.server, &uid), held, uid == owner)
                })
                .collect()
        });
        let mut list = div().flex().flex_col().gap(px(6.0));
        if rows.is_empty() {
            list = list.child(empty("search", "Nobody by that name", "Try part of their username.", p));
        }
        for (n, (m, allowed, roles, owner)) in rows.into_iter().enumerate() {
            let Some(user) = m.user.clone() else { continue };
            let name = if m.nickname.is_empty() { user_name(&user) } else { m.nickname.clone() };
            let until = timed_out_until(&m, now);
            let joined = m.joined_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
            let mut actions = div().flex().gap(px(2.0)).flex_none();
            for permission in allowed {
                let (glyph, action, color) = match permission {
                    P::TimeOutMembers => ("hourglass", Action::TimeOut(3_600), p.primary),
                    P::KickMembers => ("door-open", Action::Kick, p.destructive),
                    _ => ("gavel", Action::Ban(0), p.destructive),
                };
                let uid = user.id.clone();
                actions = actions.child(
                    icon_button_in(SharedString::from(format!("{glyph}-{}", user.id)), glyph, p, color).on_click(
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(ServerSettingsEvent::Moderate { user_id: uid.clone(), action })
                        }),
                    ),
                );
            }
            list = list.child(motion::rise(
                row(p)
                    .child(avatar(Some(&user), 40.0, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(div().font_weight(FontWeight::BOLD).text_ellipsis().child(name))
                                    .when(owner, |el| {
                                        el.child(icon("crown").size(px(14.0)).text_color(hsla(0.12, 0.9, 0.55, 1.0)))
                                    })
                                    .when(is_agent(Some(&user)), |el| {
                                        el.child(app_badge(
                                            SharedString::from(format!("members-badge|{}", user.id)),
                                            "AGENT",
                                            p,
                                        ))
                                    })
                                    .when(m.pending, |el| {
                                        el.child(pill("Hasn't agreed yet", p.muted_foreground.into()))
                                    })
                                    .when_some(until, |el, until| {
                                        el.child(pill(
                                            &format!("Timed out · {}", crate::ui::moderate::left(until - now)),
                                            amber,
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(format!("@{}", user.username))
                                    .when(joined > 0, |el| el.child("·").child(format!("joined {}", stamp(joined))))
                                    .children(roles.into_iter().map(|(role, color)| {
                                        let dot = color
                                            .map(|c| Hsla::from(gpui_kit::rgb(c)))
                                            .unwrap_or(p.muted_foreground.into());
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .px(px(7.0))
                                            .h(px(20.0))
                                            .rounded_full()
                                            .bg(p.secondary)
                                            .text_color(p.foreground)
                                            .font_weight(FontWeight::BOLD)
                                            .child(div().size(px(8.0)).rounded_full().bg(dot))
                                            .child(role)
                                    })),
                            ),
                    )
                    .child(actions),
                SharedString::from(format!("member-row-{}", user.id)),
                Duration::from_millis(25 * n.min(14) as u64),
                8.0,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(Input::new(&self.member_query).large().prefix(icon("search").size(px(16.0))))
            .child(list)
            .into_any_element()
    }

    fn bans_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().flex().flex_col().gap(px(8.0));
        match &self.bans {
            None => list = list.child(shimmer_rows(3, p)),
            Some((bans, _)) if bans.is_empty() => {
                list = list.child(empty("shield-check", "Nobody's banned", "Bans you make show up here.", p))
            }
            Some((bans, people)) => {
                for (n, ban) in bans.iter().enumerate() {
                    let user = ban.user.clone();
                    let uid = user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                    let by = people.get(&ban.banned_by_id).map(user_name).unwrap_or_else(|| "Someone".into());
                    let at = ban.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
                    list = list.child(motion::rise(
                        row(p)
                            .child(avatar(user.as_ref(), 40.0, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .child(user.as_ref().map(user_name).unwrap_or_default()),
                                    )
                                    .child(div().text_sm().child(if ban.reason.is_empty() {
                                        "No reason given".to_owned()
                                    } else {
                                        ban.reason.clone()
                                    }))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.muted_foreground)
                                            .child(format!("Banned by {by} · {}", stamp(at))),
                                    ),
                            )
                            .child(
                                soft_button(SharedString::from(format!("unban-{uid}")), "Unban", p)
                                    .child(icon("undo").size(px(14.0)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let (core, key, sid, uid) =
                                            (this.core.clone(), this.key.clone(), this.server.clone(), uid.clone());
                                        let gone = uid.clone();
                                        this.run(
                                            cx,
                                            async move { core.unban(&key, &sid, &uid).await },
                                            move |this, result, cx| {
                                                match result {
                                                    Ok(()) => {
                                                        if let Some((list, _)) = &mut this.bans {
                                                            list.retain(|b| {
                                                                b.user.as_ref().is_none_or(|u| u.id != gone)
                                                            });
                                                        }
                                                    }
                                                    Err(err) => this.error = Some(err.message),
                                                }
                                                cx.notify();
                                            },
                                        );
                                    })),
                            ),
                        SharedString::from(format!("ban-in-{n}")),
                        Duration::from_millis(30 * n.min(12) as u64),
                        8.0,
                    ));
                }
            }
        }
        list.into_any_element()
    }

    fn audit_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let filters: Vec<(i64, String)> = FILTERS.iter().map(|(a, l)| (*a as i64, (*l).to_owned())).collect();
        let picked = self.audit_action as i64;
        let channels = self
            .core
            .shared
            .read(|s| s.instance(&self.key).and_then(|i| i.channels.get(&self.server).cloned()).unwrap_or_default());
        let mut list = div().flex().flex_col().gap(px(6.0));
        match &self.audit {
            None => list = list.child(shimmer_rows(4, p)),
            Some(entries) if entries.is_empty() => {
                list = list.child(empty(
                    "scroll-text",
                    "Nothing here yet",
                    if self.audit_action == A::Unspecified {
                        "Changes to settings, channels and members show up here."
                    } else {
                        "Nothing matches this filter."
                    },
                    p,
                ))
            }
            Some(entries) => {
                for (n, entry) in entries.iter().enumerate() {
                    list = list.child(self.audit_entry(entry, n, &channels, p, cx));
                }
            }
        }
        let more = self.audit_more && self.audit.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(chips("audit-filter", &filters, picked, p, cx, |this, v, cx| {
                this.audit_action = A::try_from(v as i32).unwrap_or(A::Unspecified);
                this.audit_open = None;
                this.load_audit(false, cx);
            }))
            .child(list)
            .when(more, |el| {
                el.child(
                    div().flex().justify_center().child(
                        soft_button("audit-more", if self.audit_loading { "Loading…" } else { "Show older" }, p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_audit(true, cx))),
                    ),
                )
            })
            .into_any_element()
    }

    fn audit_entry(
        &self,
        entry: &pb::AuditEntry,
        n: usize,
        channels: &[pb::Channel],
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = entry.action();
        let (glyph, tint) = kind(action, p);
        let actor = self.audit_people.get(&entry.actor_id);
        let at = entry.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
        let details: Vec<&pb::AuditChange> = entry.changes.iter().filter(|c| c.field != "deleted_messages").collect();
        let expandable = !details.is_empty() || !entry.reason.is_empty();
        let open = expandable && self.audit_open.as_deref() == Some(entry.id.as_str());
        let id = entry.id.clone();
        let text = sentence(entry, &self.audit_people, channels);
        let head = div()
            .id(SharedString::from(format!("audit-{}", entry.id)))
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .when(expandable, |el| el.cursor_pointer())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !expandable {
                    return;
                }
                this.audit_open = if this.audit_open.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .size(px(36.0))
                    .flex_none()
                    .rounded(corner(12.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(tint.opacity(0.15))
                    .text_color(tint)
                    .child(icon(glyph).size(px(16.0)))
                    .child(
                        div()
                            .absolute()
                            .right(px(-6.0))
                            .bottom(px(-6.0))
                            .rounded_full()
                            .border_2()
                            .border_color(p.card)
                            .child(avatar(actor, 18.0, p)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        crate::ui::text::markdown(SharedString::from(format!("audit-text-{}", entry.id)), text)
                            .w_full(),
                    )
                    .child(div().text_xs().text_color(p.muted_foreground).child(stamp(at))),
            )
            .when(expandable, |el| {
                el.child(
                    icon(if open { "chevron-up" } else { "chevron-down" })
                        .size(px(16.0))
                        .text_color(p.muted_foreground),
                )
            });
        let mut item = div()
            .rounded(corner(16.0))
            .border_1()
            .border_color(if open { alpha(p.primary, 0.4) } else { p.border.into() })
            .bg(if open { alpha(p.muted_foreground, 0.06) } else { p.card.into() })
            .overflow_hidden()
            .child(head);
        if open {
            let one_side = match action {
                A::InviteCreate | A::AutoModRuleCreate | A::EmojiCreate | A::WebhookCreate => Some(true),
                A::InviteDelete | A::AutoModRuleDelete | A::EmojiDelete | A::WebhookDelete => Some(false),
                _ => None,
            };
            let mut more = div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(14.0))
                .py(px(10.0))
                .pl(px(60.0))
                .border_t_1()
                .border_color(p.border)
                .text_sm();
            if !entry.reason.is_empty() {
                more = more.child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(div().text_color(p.muted_foreground).child("Reason:"))
                        .child(entry.reason.clone()),
                );
            }
            let green = hsla(0.42, 0.6, if p.dark { 0.6 } else { 0.36 }, 1.0);
            for (k, change) in details.into_iter().enumerate() {
                let label = field_label(&change.field);
                let before = value(&change.field, &change.before, entry, &self.audit_people, channels);
                let after = value(&change.field, &change.after, entry, &self.audit_people, channels);
                let line = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().text_color(p.muted_foreground).child(format!("{label}:")));
                let line = match one_side {
                    Some(after_side) => line.child(chip_text(
                        if after_side { after } else { before },
                        p.foreground.into(),
                        p.secondary.into(),
                    )),
                    None => line
                        .child(chip_text(before, p.destructive.into(), alpha(p.destructive, 0.1)).line_through())
                        .child(icon("arrow-right").size(px(13.0)).text_color(p.muted_foreground))
                        .child(chip_text(after, green, green.opacity(0.12))),
                };
                more = more.child(motion::rise(
                    line,
                    SharedString::from(format!("change-{}-{k}", entry.id)),
                    Duration::from_millis(40 * k as u64),
                    4.0,
                ));
            }
            item = item.child(more);
        }
        motion::slide_in(item, SharedString::from(format!("audit-in-{}-{n}", entry.id)), -12.0).into_any_element()
    }
}

impl Render for ServerSettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let (server, access) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (i.and_then(|i| i.server(&self.server).cloned()), i.map(|i| i.access(&self.server)).unwrap_or_default())
        });
        let Some(server) = server else {
            // The server went away (left, kicked or deleted).
            cx.defer_in(window, |_, _, cx| cx.emit(ServerSettingsEvent::Close));
            return div().into_any_element();
        };
        let allowed = pages(&access);
        // Requests waiting on this server's approval, counted on the menu once the list is read.
        let requests = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.shared.get(&self.server))
                .map_or(0, |l| l.connections.iter().filter(|c| c.home && crate::core::shared::waiting(c)).count())
        });
        if allowed.contains(&Page::Shared) {
            self.load_shared(cx);
        }
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

        let mut menu = div().flex().flex_col().w(px(220.0)).child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(10.0))
                .pb(px(14.0))
                .child(server_icon(&server, 32.0, 10.0, &p))
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(server.name.clone()),
                ),
        );
        let mut y = 46.0;
        let mut at_y = 0.0;
        for (group, list) in [("SERVER SETTINGS", &SETTINGS[..]), ("MODERATION", &MODERATION[..])] {
            let shown: Vec<Page> = list.iter().copied().filter(|pg| allowed.contains(pg)).collect();
            if shown.is_empty() {
                continue;
            }
            menu = menu.child(
                div()
                    .h(px(30.0))
                    .px(px(10.0))
                    .when(y > 46.0, |el| el.mt(px(14.0)))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(group),
            );
            y += if y > 46.0 { 44.0 } else { 30.0 };
            for pg in shown {
                let on = pg == page;
                if on {
                    at_y = y;
                }
                let hover = alpha(p.primary, 0.08);
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("smenu-{}", pg.label())))
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
                        .child(if pg == Page::Shared {
                            crate::ui::shared_marks::glyph(17.0, if on { p.primary } else { p.muted_foreground })
                                .into_any_element()
                        } else {
                            icon(pg.glyph())
                                .size(px(17.0))
                                .text_color(if on { p.primary } else { p.muted_foreground })
                                .into_any_element()
                        })
                        .child(div().flex_1().child(pg.label()))
                        .when(pg == Page::Shared && requests > 0, |el| {
                            el.child(crate::ui::widgets::badge(requests as u32, &p))
                        }),
                );
                y += 40.0;
            }
        }
        let at = motion::follow("server-settings-hl", at_y, window, cx);
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
        let body = match page {
            Page::Overview => self.overview(&server, &p, window, cx),
            Page::Invites => self.invites_page(&p, cx),
            Page::Welcome => self.welcome_page(&server, &p, window, cx),
            Page::Roles => self.roles_page(&p, window, cx),
            Page::Channels => self.channels_page(&p, window, cx),
            Page::Emoji => self.emoji_page(&p, window, cx),
            Page::Integrations => {
                let mut both = div().flex().flex_col().gap(px(36.0));
                if access.has(P::ManageServer) {
                    both = both.child(self.agents_page(&p, window, cx));
                }
                if access.has(P::ManageWebhooks) {
                    both = both.child(self.webhooks_page(&p, window, cx));
                }
                both.into_any_element()
            }
            Page::Shared => self.shared_page(&p, window, cx),
            Page::Members => self.members_page(&p, cx),
            Page::Bans => self.bans_page(&p, cx),
            Page::AutoMod => self.automod_page(&p, window, cx),
            Page::AuditLog => self.audit_page(&p, cx),
        };
        let content = div()
            .w(px(if matches!(page, Page::Roles | Page::Welcome | Page::Channels) { 860.0 } else { 680.0 }))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(page.label()))
            .child(div().text_color(p.muted_foreground).child(page.about()))
            .child(div().h(px(18.0)))
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(div().mb(px(12.0)).child(e)))
            .child(body);

        motion::fade_in(
            div()
                .id("server-settings")
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
                        .child(motion::slide_in(menu, "server-settings-menu", -24.0)),
                )
                .child(
                    div()
                        .id("server-settings-body")
                        .flex_1()
                        .h_full()
                        .overflow_y_scroll()
                        .pt(px(56.0))
                        .px(px(40.0))
                        .pb(px(40.0))
                        .child(motion::rise(
                            content,
                            SharedString::from(format!("spage-{}", page.label())),
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
                                .id("server-settings-close")
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
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(ServerSettingsEvent::Close)))
                                .child(icon("x").size(px(18.0))),
                        )
                        .child(
                            div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"),
                        ),
                ),
            "server-settings-in",
            Duration::from_millis(160),
        )
        .into_any_element()
    }
}

/// A member, what you may do to them, their roles (name and color), and whether they own the server.
type MemberRow = (pb::Member, Vec<P>, Vec<(String, Option<u32>)>, bool);

pub(crate) fn amber(p: &Palette) -> Hsla {
    hsla(0.11, 0.9, if p.dark { 0.62 } else { 0.42 }, 1.0)
}

/// A card-like row in a list.
fn row(p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .p(px(12.0))
        .rounded(corner(16.0))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
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

fn chip_text(text: String, fg: Hsla, bg: Hsla) -> gpui_kit::Div {
    div().px(px(6.0)).rounded(px(6.0)).bg(bg).text_color(fg).child(text)
}

/// The floating "n changes not saved" bar, with Discard and Save.
pub(crate) fn save_bar<V: 'static>(
    id: &str,
    n: usize,
    saving: bool,
    p: &Palette,
    cx: &mut Context<V>,
    discard: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    save: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    motion::rise(
        div()
            .id(SharedString::from(format!("{id}-card")))
            .occlude()
            .w(px(560.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(10.0))
            .rounded(corner(16.0))
            .bg(p.card)
            .border_1()
            .border_color(alpha(p.primary, 0.4))
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, if p.dark { 0.3 } else { 0.2 }),
                offset: gpui_kit::point(px(0.0), px(16.0)),
                blur_radius: px(40.0),
                spread_radius: px(-10.0),
                inset: false,
            }])
            .child(div().flex_1().text_sm().font_weight(FontWeight::BOLD).child(if n == 1 {
                "1 change not saved".to_owned()
            } else {
                format!("{n} changes not saved")
            }))
            .child(
                soft_button(SharedString::from(format!("{id}-discard")), "Discard", p)
                    .on_click(cx.listener(move |this, _, window, cx| discard(this, window, cx))),
            )
            .child(
                primary_button(
                    SharedString::from(format!("{id}-save")),
                    if saving { "Saving…" } else { "Save changes" },
                    p,
                )
                .when(saving, |el| el.opacity(0.6))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !saving {
                        save(this, window, cx)
                    }
                })),
            ),
        SharedString::from(id.to_owned()),
        Duration::ZERO,
        12.0,
    )
    .into_any_element()
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

fn empty(glyph: &str, title: &str, body: &str, p: &Palette) -> impl IntoElement {
    motion::rise(
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .py(px(48.0))
            .child(icon(glyph).size(px(32.0)).text_color(p.muted_foreground))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
            .child(div().text_sm().text_color(p.muted_foreground).child(body.to_owned())),
        SharedString::from(format!("empty-{title}")),
        Duration::ZERO,
        8.0,
    )
}

/// Choices as pills, the picked one filled; it pops each time it changes.
fn chips(
    id: &'static str,
    options: &[(i64, String)],
    picked: i64,
    p: &Palette,
    cx: &mut Context<ServerSettingsView>,
    pick: fn(&mut ServerSettingsView, i64, &mut Context<ServerSettingsView>),
) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .children(options.iter().map(|(value, label)| {
            let (on, value) = (*value == picked, *value);
            chip(SharedString::from(format!("{id}-{value}")), label, on, p)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !on {
                        pick(this, value, cx)
                    }
                }))
                .into_any_element()
        }))
        .into_any_element()
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

/// An audit entry's icon and tint.
fn kind(action: A, p: &Palette) -> (&'static str, Hsla) {
    let sky = hsla(0.55, 0.85, 0.5, 1.0);
    let green = hsla(0.42, 0.65, 0.42, 1.0);
    let violet = hsla(0.74, 0.7, 0.62, 1.0);
    let amber = amber(p);
    let orange = hsla(0.07, 0.9, 0.55, 1.0);
    let pink = hsla(0.92, 0.8, 0.6, 1.0);
    let red: Hsla = p.destructive.into();
    match action {
        A::ServerUpdate => ("settings", sky),
        A::ChannelCreate => ("folder-plus", green),
        A::ChannelUpdate => ("hash", sky),
        A::ChannelDelete => ("trash", red),
        A::ChannelsReorder => ("arrow-down-up", sky),
        A::ChannelPermissionsUpdate => ("lock", sky),
        A::RoleCreate => ("shield-plus", green),
        A::RoleUpdate => ("shield", violet),
        A::RoleDelete => ("shield-x", red),
        A::RolesReorder => ("arrow-down-up", violet),
        A::MemberRolesUpdate | A::MemberUpdate => ("user-cog", violet),
        A::MemberTimeOut => ("hourglass", amber),
        A::MemberKick => ("door-open", orange),
        A::MemberBan => ("gavel", red),
        A::MemberUnban => ("undo", green),
        A::MessageDelete => ("message-square-x", red),
        A::OwnershipTransfer => ("crown", amber),
        A::InviteCreate => ("link", green),
        A::InviteDelete => ("link-2-off", red),
        A::ApplicationApprove => ("user-check", green),
        A::ApplicationReject => ("user-x", red),
        A::JoinFormUpdate => ("clipboard-list", sky),
        A::WelcomeScreenUpdate => ("party-popper", pink),
        A::AutoModRuleCreate => ("shield-check", green),
        A::AutoModRuleUpdate => ("shield-alert", sky),
        A::AutoModRuleDelete => ("shield-x", red),
        A::AutoModTimeOut => ("bot", amber),
        A::AutoModMessageDelete => ("bot", red),
        A::EmojiCreate => ("face-slightly-smiling-plus", green),
        A::EmojiUpdate => ("face-slightly-smiling", sky),
        A::EmojiDelete => ("face-slightly-frowning", red),
        A::WebhookCreate => ("webhook", green),
        A::WebhookUpdate => ("webhook", sky),
        A::WebhookDelete => ("unplug", red),
        A::AgentAdd => ("bot", violet),
        A::ShareCodeCreate | A::SharedChannelRequest => ("link", green),
        A::ShareCodeDelete => ("link-2-off", red),
        A::SharedChannelApprove => ("check", green),
        A::SharedChannelUpdate => ("settings", sky),
        A::SharedChannelDisconnect => ("unplug", red),
        A::SharedChannelBlock => ("user-x", red),
        A::SharedChannelUnblock => ("undo", green),
        A::Unspecified => ("scroll-text", p.muted_foreground.into()),
    }
}

fn field_label(field: &str) -> String {
    match field {
        "name" => "Name",
        "description" => "Description",
        "icon_url" => "Icon",
        "discoverable" => "Shown in Browse",
        "default_notifications" => "Default notifications",
        "system_channel_id" => "Join messages",
        "topic" => "Topic",
        "parent_id" => "Category",
        "position" => "Position",
        "slowmode_seconds" => "Slow mode",
        "nickname" => "Nickname",
        "role" => "Role",
        "timed_out_until" => "Timed out until",
        "owner_id" => "Owner",
        "color" => "Color",
        "permissions" => "Permissions",
        "hoist" => "Shown apart",
        "mentionable" => "Anyone can mention it",
        "max_uses" => "How many people",
        "expires_at" => "Expires",
        "uses" => "People it let in",
        "min_account_age_seconds" => "Minimum account age",
        "applications" => "Apply to join",
        "linked_only" => "waifu.dev accounts only",
        "rules" => "Rules",
        "avatar_url" => "Picture",
        "channel_id" => "Posts in",
        "token" => "Address",
        "questions" => "Questions",
        "enabled" => "On",
        "channels" => "Channels",
        "keywords" => "Words",
        "allowed" => "Allowed",
        "mention_limit" => "Ping limit",
        "actions" => "Actions",
        other => other,
    }
    .to_owned()
}

/// A value from the log, in words.
fn value(field: &str, raw: &str, entry: &pb::AuditEntry, people: &People, channels: &[pb::Channel]) -> String {
    let yes_no = |raw: &str| if raw == "true" { "Yes" } else { "No" }.to_owned();
    let at = entry.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
    match field {
        "discoverable" | "enabled" | "hoist" | "mentionable" | "applications" | "linked_only" => yes_no(raw),
        "role" => match raw {
            "1" => "Member".into(),
            "2" => "Admin".into(),
            "3" => "Owner".into(),
            "" => "None".into(),
            _ => entry.role_name.clone(),
        },
        "color" if raw.is_empty() => "None".into(),
        "permissions" => {
            let names: Vec<String> = raw
                .split(',')
                .filter_map(|n| n.parse::<i32>().ok())
                .filter_map(|n| P::try_from(n).ok())
                .map(|p| words(&format!("{p:?}")))
                .collect();
            if names.is_empty() { "None".into() } else { names.join(", ") }
        }
        "default_notifications" => {
            match raw.parse::<i32>().ok().and_then(|n| pb::NotificationLevel::try_from(n).ok()) {
                Some(pb::NotificationLevel::Mentions) => "Only @mentions".into(),
                Some(pb::NotificationLevel::All) => "All messages".into(),
                _ => "Each person's own".into(),
            }
        }
        "avatar_url" | "icon_url" => if raw.is_empty() { "None" } else { "A picture" }.into(),
        "token" => "Replaced".into(),
        "system_channel_id" | "parent_id" | "channel_id" => {
            if raw.is_empty() {
                return "None".into();
            }
            match channels.iter().find(|c| c.id == raw) {
                Some(c) if field == "parent_id" => c.name.clone(),
                Some(c) => format!("#{}", c.name),
                None => "A deleted channel".into(),
            }
        }
        "slowmode_seconds" => match raw.parse::<i64>().unwrap_or(0) {
            0 => "Off".into(),
            s => duration(s),
        },
        "timed_out_until" => match raw.parse::<i64>() {
            Ok(until) if until > 0 => format!("{} ({})", stamp(until), duration((until - at).max(0) / 1000)),
            _ => "Not timed out".into(),
        },
        "owner_id" => people.get(raw).map(user_name).unwrap_or_else(|| "Someone".into()),
        "max_uses" if raw == "0" => "No limit".into(),
        "expires_at" => match raw.parse::<i64>() {
            Ok(ms) if ms > 0 => stamp(ms),
            _ => "Never".into(),
        },
        "min_account_age_seconds" => match raw.parse::<i64>().unwrap_or(0) {
            0 => "Any age".into(),
            s => duration(s),
        },
        _ if raw.is_empty() => "Nothing".into(),
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
    let who = |id: &str| format!("**{}**", plain(&people.get(id).map(user_name).unwrap_or_else(|| "Someone".into())));
    let actor = who(&entry.actor_id);
    let target = who(&entry.target_id);
    let change = |field: &str| entry.changes.iter().find(|c| c.field == field);
    let shared_server = || change("server").map(|c| c.after.clone()).unwrap_or_else(|| "another server".into());
    let at = entry.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
    let named_channel = |name: &str| format!("**#{}**", plain(name));
    let channel = match channels.iter().find(|c| c.id == entry.target_id) {
        Some(c) if c.r#type == pb::ChannelType::Category as i32 => format!("**{}**", plain(&c.name)),
        Some(c) => named_channel(&c.name),
        None => named_channel(&entry.channel_name),
    };
    let role = format!("**{}**", plain(if entry.role_name.is_empty() { "a role" } else { &entry.role_name }));
    let only = |field: &str| entry.changes.len() == 1 && change(field).is_some();
    let name_of = |side_after: bool| {
        change("name").map(|c| plain(if side_after { &c.after } else { &c.before })).unwrap_or_default()
    };
    match entry.action() {
        A::ServerUpdate => {
            if only("applications") {
                if change("applications").is_some_and(|c| c.after == "true") {
                    format!("{actor} made people apply to join")
                } else {
                    format!("{actor} let people join without applying")
                }
            } else if only("discoverable") {
                if change("discoverable").is_some_and(|c| c.after == "true") {
                    format!("{actor} listed the server in Browse")
                } else {
                    format!("{actor} made the server invite only")
                }
            } else if only("name") {
                format!("{actor} renamed the server to **{}**", name_of(true))
            } else {
                format!("{actor} changed the server's settings")
            }
        }
        A::ChannelCreate => format!("{actor} created {channel}"),
        A::ChannelUpdate => match change("slowmode_seconds").filter(|_| entry.changes.len() == 1) {
            Some(c) if c.after == "0" => format!("{actor} turned off slow mode in {channel}"),
            Some(c) => format!("{actor} set slow mode in {channel} to {}", duration(c.after.parse().unwrap_or(0))),
            None => format!("{actor} changed {channel}"),
        },
        A::ChannelDelete => format!("{actor} deleted {}", named_channel(&entry.channel_name)),
        A::ChannelsReorder => format!("{actor} rearranged the channels"),
        A::MemberUpdate => format!("{actor} changed {target}'s nickname"),
        A::MemberRolesUpdate => match change("role") {
            Some(c) if !c.after.is_empty() => format!("{actor} gave {target} {role}"),
            _ => format!("{actor} took {role} from {target}"),
        },
        A::RoleCreate => format!("{actor} created the role {role}"),
        A::RoleUpdate => {
            if only("name") {
                format!("{actor} renamed **{}** to **{}**", name_of(false), name_of(true))
            } else if only("permissions") {
                format!("{actor} changed what {role} can do")
            } else {
                format!("{actor} changed {role}")
            }
        }
        A::RoleDelete => format!("{actor} deleted the role {role}"),
        A::RolesReorder => format!("{actor} rearranged the roles"),
        A::ChannelPermissionsUpdate => format!("{actor} changed who can do what in {channel}"),
        A::MemberTimeOut => match change("timed_out_until").and_then(|c| c.after.parse::<i64>().ok()) {
            Some(until) if until > 0 => {
                format!("{actor} timed out {target} for {}", duration(((until - at) / 1000).max(1)))
            }
            _ => format!("{actor} ended {target}'s time-out"),
        },
        A::MemberKick => format!("{actor} kicked {target}"),
        A::MemberBan => {
            let deleted: i64 = change("deleted_messages").and_then(|c| c.after.parse().ok()).unwrap_or(0);
            match deleted {
                0 => format!("{actor} banned {target}"),
                1 => format!("{actor} banned {target} and deleted 1 message"),
                n => format!("{actor} banned {target} and deleted {n} messages"),
            }
        }
        A::MemberUnban => format!("{actor} unbanned {target}"),
        A::MessageDelete => {
            format!("{actor} deleted a message by {target} in {}", named_channel(&entry.channel_name))
        }
        A::OwnershipTransfer => format!("{actor} handed the server to {target}"),
        A::InviteCreate if !entry.channel_name.is_empty() => {
            format!("{actor} made an invite to {}", named_channel(&entry.channel_name))
        }
        A::InviteCreate => format!("{actor} made an invite"),
        A::InviteDelete if !entry.channel_name.is_empty() => {
            format!("{actor} revoked an invite to {}", named_channel(&entry.channel_name))
        }
        A::InviteDelete => format!("{actor} revoked an invite"),
        A::ApplicationApprove => format!("{actor} let {target} in"),
        A::ApplicationReject => format!("{actor} turned down {target}'s application"),
        A::JoinFormUpdate => match (change("rules").is_some(), change("questions").is_some()) {
            (true, false) => format!("{actor} changed the rules"),
            (false, true) => format!("{actor} changed the questions"),
            _ => format!("{actor} changed the rules and questions"),
        },
        A::WelcomeScreenUpdate => match change("enabled").filter(|_| entry.changes.len() == 1) {
            Some(c) if c.after == "true" => format!("{actor} turned on the welcome screen"),
            Some(_) => format!("{actor} turned off the welcome screen"),
            None => format!("{actor} changed the welcome screen"),
        },
        A::AutoModRuleCreate => format!("{actor} added the AutoMod rule **{}**", name_of(true)),
        A::AutoModRuleUpdate => format!("{actor} changed the AutoMod rule **{}**", name_of(true)),
        A::AutoModRuleDelete => format!("{actor} deleted the AutoMod rule **{}**", name_of(false)),
        A::AutoModTimeOut => {
            let mut s = format!("**AutoMod** timed out {target}");
            if let Some(until) = change("timed_out_until").and_then(|c| c.after.parse::<i64>().ok()) {
                s.push_str(&format!(" for {}", duration(((until - at) / 1000).max(1))));
            }
            if !entry.channel_name.is_empty() {
                s.push_str(&format!(" in {}", named_channel(&entry.channel_name)));
            }
            s
        }
        A::AutoModMessageDelete => {
            let mut s = format!("**AutoMod** took down a message from {target}");
            if !entry.channel_name.is_empty() {
                s.push_str(&format!(" in {}", named_channel(&entry.channel_name)));
            }
            s
        }
        A::EmojiCreate => format!("{actor} added the emoji **:{}:**", name_of(true)),
        A::EmojiUpdate => format!("{actor} renamed **:{}:** to **:{}:**", name_of(false), name_of(true)),
        A::EmojiDelete => format!("{actor} deleted the emoji **:{}:**", name_of(false)),
        A::WebhookCreate => {
            format!("{actor} made the webhook **{}** for {}", name_of(true), named_channel(&entry.channel_name))
        }
        A::WebhookUpdate => format!("{actor} changed the webhook **{}**", name_of(true)),
        A::WebhookDelete => format!("{actor} deleted the webhook **{}**", name_of(false)),
        A::AgentAdd => format!("{actor} added the agent {target}"),
        A::ShareCodeCreate => format!("{actor} made a share code for {}", named_channel(&entry.channel_name)),
        A::ShareCodeDelete => format!("{actor} deleted a share code for {}", named_channel(&entry.channel_name)),
        A::SharedChannelRequest => {
            format!("{actor} asked to show {} from **{}**", named_channel(&entry.channel_name), plain(&shared_server()))
        }
        A::SharedChannelApprove => {
            format!("{actor} shared {} with **{}**", named_channel(&entry.channel_name), plain(&shared_server()))
        }
        A::SharedChannelUpdate => {
            format!("{actor} changed what the other server may do in {}", named_channel(&entry.channel_name))
        }
        A::SharedChannelDisconnect => {
            format!("{actor} ended sharing {} with **{}**", named_channel(&entry.channel_name), plain(&shared_server()))
        }
        A::SharedChannelBlock => format!("{actor} kept {target} out of {}", named_channel(&entry.channel_name)),
        A::SharedChannelUnblock => format!("{actor} let {target} back into {}", named_channel(&entry.channel_name)),
        A::Unspecified => format!("{actor} did something"),
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
        assert!(pages(&nobody).is_empty());
        let channels = std::iter::once(("general".to_owned(), u32::MAX)).collect();
        let owner = Access { owner: true, server: u32::MAX, channels, ..Access::default() };
        assert_eq!(words("ManageServer"), "Manage server");
        assert_eq!(
            pages(&owner),
            vec![
                Page::Overview,
                Page::Welcome,
                Page::Invites,
                Page::Roles,
                Page::Channels,
                Page::Emoji,
                Page::Integrations,
                Page::Shared,
                Page::Members,
                Page::Bans,
                Page::AutoMod,
                Page::AuditLog
            ]
        );
        let hooks = Access { server: crate::core::permissions::bit(P::ManageWebhooks), ..Access::default() };
        assert_eq!(pages(&hooks), vec![Page::Integrations]);
        // Managing one channel opens the Channels page.
        let one = std::iter::once(("general".to_owned(), crate::core::permissions::bit(P::ManageRoles))).collect();
        let keeper = Access { channels: one, ..Access::default() };
        assert_eq!(pages(&keeper), vec![Page::Channels]);
    }
}
