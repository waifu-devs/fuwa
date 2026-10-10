//! Overview and Access, as in the web's `ServerSettingsDialog.tsx`: the
//! server's name, icon, description, join messages and default
//! notifications; who can join, whether they apply first, waifu.dev
//! accounts only and how old an account must be. Both are drafts saved from
//! the floating bar, beside previews of the server as Browse shows it, its
//! join message, and three accounts at the door.

use gpui_kit::{ObjectFit, StyledImage as _, img};

use super::pages::{boxed, focused, heading, label, part, preview_card};
use super::*;
use crate::core::pictures::PictureKind;
use crate::core::server_pages::{ACCOUNT_AGES, AccessPatch};
use crate::ui::motion::{counted, swapping};
use crate::ui::settings_controls::{Look, Opt, button, chips, choice, toggle};
use crate::ui::theme::{radius_lg, radius_xl};

pub(super) struct Overview {
    /// The icon not saved yet; `None` is the server's.
    icon: Option<String>,
    icon_link: Entity<InputState>,
    link_open: bool,
    uploading: bool,
    picture_error: Option<String>,
    /// The channel for join messages not saved yet ("" for none).
    system: Option<String>,
    mentions: Option<bool>,
    /// The join messages menu is open.
    menu: bool,
    saving: bool,
    save_error: Option<String>,
    /// Access: what's picked but not saved.
    access: AccessPatch,
    access_saving: bool,
    access_error: Option<String>,
}

impl Overview {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let icon_link = cx.new(|cx| InputState::new(window, cx).placeholder("https://…"));
        let subscriptions = vec![cx.subscribe(&icon_link, |this: &mut ServerSettingsView, s, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                let typed = s.read(cx).value().to_string();
                this.pages.overview.icon = Some(typed);
                cx.notify();
            }
        })];
        let overview = Self {
            icon: None,
            icon_link,
            link_open: false,
            uploading: false,
            picture_error: None,
            system: None,
            mentions: None,
            menu: false,
            saving: false,
            save_error: None,
            access: AccessPatch::default(),
            access_saving: false,
            access_error: None,
        };
        (overview, subscriptions)
    }
}

fn mentions_only(server: &pb::Server) -> bool {
    server.default_notifications == pb::NotificationLevel::Mentions as i32
}

/// A link someone could have typed for a picture: empty, or http(s) without spaces.
fn is_link(value: &str) -> bool {
    let v = value.trim();
    v.is_empty() || ((v.starts_with("https://") || v.starts_with("http://")) && !v.contains(char::is_whitespace))
}

impl ServerSettingsView {
    /// The overview's edits: (name, icon, description, join channel, mentions only), each `None` when unchanged.
    fn overview_draft(&self, server: &pb::Server, cx: &Context<Self>) -> ServerPatch {
        let o = &self.pages.overview;
        let name = self.name.read(cx).value().to_string();
        let about = self.description.read(cx).value().to_string();
        ServerPatch {
            name: (name != server.name).then_some(name),
            description: (about != server.description).then_some(about),
            icon_url: o.icon.clone().filter(|i| *i != server.icon_url),
            system_channel_id: o.system.clone().filter(|c| *c != server.system_channel_id),
            default_notifications: o
                .mentions
                .filter(|m| *m != mentions_only(server))
                .map(|m| if m { pb::NotificationLevel::Mentions } else { pb::NotificationLevel::Unspecified }),
            ..ServerPatch::default()
        }
    }

    fn discard_overview(&mut self, server: &pb::Server, window: &mut Window, cx: &mut Context<Self>) {
        let (name, about) = (server.name.clone(), server.description.clone());
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.description.update(cx, |s, cx| s.set_value(about, window, cx));
        let o = &mut self.pages.overview;
        o.icon = None;
        o.system = None;
        o.mentions = None;
        o.save_error = None;
        o.picture_error = None;
        o.icon_link.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn save_overview(&mut self, server: &pb::Server, cx: &mut Context<Self>) {
        let mut patch = self.overview_draft(server, cx);
        if let Some(name) = &patch.name {
            if name.trim().is_empty() {
                self.pages.overview.save_error = Some(t("serversettings.overview.needsName"));
                cx.notify();
                return;
            }
            patch.name = Some(name.trim().to_owned());
        }
        if let Some(icon) = &patch.icon_url {
            if !icon.trim().is_empty() && !is_link(icon) {
                self.pages.overview.save_error = Some(t("serversettings.overview.iconLink"));
                cx.notify();
                return;
            }
            patch.icon_url = Some(icon.trim().to_owned());
        }
        patch.description = patch.description.map(|d| d.trim().to_owned());
        self.pages.overview.saving = true;
        self.pages.overview.save_error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.update_server(&key, &sid, patch).await }, |this, result, cx| {
            let o = &mut this.pages.overview;
            o.saving = false;
            match result {
                Ok(server) => {
                    o.icon = None;
                    o.system = None;
                    o.mentions = None;
                    o.link_open = false;
                    // What the server trimmed shows as saved.
                    this.filled = false;
                    let _ = server;
                }
                Err(err) => o.save_error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Asks the system for a picture, frames it, and uploads it as the icon not saved yet.
    fn pick_icon(&mut self, cx: &mut Context<Self>) {
        if self.pages.overview.uploading || self.cropper.is_some() {
            return;
        }
        crate::ui::cropper::choose(
            self.core.clone(),
            PictureKind::Icon,
            t("desktop.account.choosePicture"),
            cx,
            |this| &mut this.cropper,
            |this, error, cx| {
                this.pages.overview.picture_error = Some(error);
                cx.notify();
            },
            Rc::new(|this: &mut Self, bytes, mime, cx: &mut Context<Self>| this.upload_icon(bytes, mime, cx)),
        );
    }

    fn upload_icon(&mut self, bytes: Vec<u8>, mime: &'static str, cx: &mut Context<Self>) {
        self.pages.overview.uploading = true;
        self.pages.overview.picture_error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(
            cx,
            async move { core.upload_picture(&key, pb::MediaPurpose::ServerIcon, mime, bytes).await },
            |this, result, cx| {
                let o = &mut this.pages.overview;
                o.uploading = false;
                match result {
                    Ok(url) => {
                        o.icon = Some(url);
                        o.link_open = false;
                    }
                    Err(err) => o.picture_error = Some(crate::ui::instance_home::capitalized(&err.message)),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn overview(
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
        let o = &self.pages.overview;
        let icon_url = o.icon.clone().unwrap_or_else(|| server.icon_url.clone());
        let system = o.system.clone().unwrap_or_else(|| server.system_channel_id.clone());
        let mentions = o.mentions.unwrap_or_else(|| mentions_only(server));
        let draft = self.overview_draft(server, cx);
        let changes = [
            draft.name.is_some(),
            draft.icon_url.is_some(),
            draft.description.is_some(),
            draft.system_channel_id.is_some(),
            draft.default_notifications.is_some(),
        ]
        .into_iter()
        .filter(|c| *c)
        .count();
        if changes > 0 {
            let (s1, s2) = (server.clone(), server.clone());
            self.bar = Some(bar_with_error(
                "overview-bar",
                changes,
                self.pages.overview.saving,
                self.pages.overview.save_error.as_deref(),
                p,
                cx,
                move |this, window, cx| this.discard_overview(&s1, window, cx),
                move |this, _, cx| this.save_overview(&s2, cx),
            ));
        }
        let (channels, me, regions) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (
                i.and_then(|i| i.channels.get(&self.server))
                    .into_iter()
                    .flatten()
                    .filter(|c| {
                        c.r#type == pb::ChannelType::Text as i32 || c.r#type == pb::ChannelType::Announcement as i32
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
                i.and_then(|i| i.me.clone()),
                i.and_then(|i| i.node.as_ref().map(|n| n.regions.clone())).unwrap_or_default(),
            )
        });
        let greeting = channels.iter().find(|c| c.id == system).map(|c| c.name.clone());
        let name_now = self.name.read(cx).value().to_string();
        let mut shown = server.clone();
        if !name_now.is_empty() {
            shown.name = name_now;
        }
        shown.icon_url = if is_link(&icon_url) { icon_url.trim().to_owned() } else { String::new() };
        shown.description = self.description.read(cx).value().to_string();

        let form_w = if self.wide { self.column - 288.0 - 40.0 } else { self.column };
        let name_part = part(true, false, 8.0, p).child(label(&t("serversettings.overview.name"))).child(boxed(
            Input::new(&self.name).appearance(false),
            44.0,
            focused(&self.name, window, cx),
            p,
        ));
        let icon_part = part(false, false, 8.0, p)
            .child(
                heading(&t("serversettings.overview.icon"), Some(&t("serversettings.overview.iconHint")), p)
                    .gap(px(8.0)),
            )
            .child(self.icon_field(server, &shown, &icon_url, p, window, cx));
        let about_part = part(false, false, 8.0, p)
            .child(label(&t("serversettings.nav.description")))
            .child(crate::ui::instance_home::focus_ring(
                div()
                    .w_full()
                    .min_h(px(62.0))
                    // The box keeps its own padding, so this brings the words to the web's 12 and 8.
                    .px(px(2.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .text_sm()
                    .child(Textarea::new(&self.description).appearance(false)),
                focused(&self.description, window, cx),
                p,
            ))
            .child(
                div()
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(t("serversettings.overview.descriptionHint")),
            );
        let region_part = (regions.len() > 1).then(|| {
            let name = crate::core::instance_servers::region_name(&regions, &server.region);
            part(false, false, 12.0, p)
                .flex_row()
                .items_center()
                .child(
                    div()
                        .size(px(40.0))
                        .flex_none()
                        .rounded(radius_xl())
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .child(icon("map-pin").size(px(20.0))),
                )
                .child(heading(
                    &t_with("serversettings.overview.region", &[("region", Arg::Str(&name))]),
                    Some(&t("serversettings.overview.regionHint")),
                    p,
                ))
        });
        let joins_part = part(false, false, 8.0, p)
            .child(
                heading(&t("serversettings.nav.joinMessages"), Some(&t("serversettings.overview.joinMessagesHint")), p)
                    .gap(px(8.0)),
            )
            .child(self.join_menu(&channels, &system, greeting.as_deref(), p, cx));
        let notify_part = part(false, true, 12.0, p)
            .child(heading(
                &t("serversettings.nav.defaultNotifications"),
                Some(&t("serversettings.overview.defaultNotificationsHint")),
                p,
            ))
            .child(choice(
                "default-notifications",
                Some(usize::from(mentions)),
                vec![
                    Opt::new(
                        t("serversettings.overview.ownSetting"),
                        t("serversettings.overview.ownSettingHint"),
                        "bell",
                    ),
                    Opt::new(t("common.notify.mentions"), t("serversettings.overview.mentionsHint"), "at-sign"),
                ],
                form_w,
                p,
                window,
                cx,
                |this: &mut Self, i, cx| {
                    this.pages.overview.mentions = Some(i == 1);
                    cx.notify();
                },
            ));
        let form = div()
            .flex()
            .flex_col()
            .child(self.mark("name", name_part, p))
            .child(self.mark("icon", icon_part, p))
            .child(self.mark("description", about_part, p))
            .children(region_part)
            .child(self.mark("join-messages", joins_part, p))
            .child(self.mark("default-notifications", notify_part, p));
        let preview = div().flex().flex_col().gap(px(16.0)).child(browse_card(&shown, p)).child(join_preview(
            greeting.as_deref(),
            me.as_ref(),
            p,
        ));
        crate::ui::settings_controls::with_preview(form, preview, self.wide, p)
    }

    /// The icon (the web's `PictureField` for a server icon): the tile, Upload or Change, Remove,
    /// and a link instead.
    fn icon_field(
        &mut self,
        server: &pb::Server,
        shown: &pb::Server,
        value: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let o = &self.pages.overview;
        let busy = o.uploading;
        let link_open = o.link_open || !is_link(value);
        let mut bare = shown.clone();
        bare.icon_url = String::new();
        let radius = 80.0 * 0.32;
        let tile = div()
            .id("server-icon-tile")
            .group("server-icon-tile")
            .relative()
            .size(px(80.0))
            .flex_none()
            .rounded(px(radius))
            .overflow_hidden()
            .border_1()
            .border_color(p.border)
            .bg(p.muted)
            .cursor_pointer()
            .active(|s| s.opacity(0.9))
            .on_click(cx.listener(|this, _, _, cx| this.pick_icon(cx)))
            .child(if !shown.icon_url.is_empty() {
                crate::ui::widgets::picture(shown.icon_url.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            } else {
                motion::once(
                    server_icon(&bare, 78.0, radius, p).text_size(px(24.0)),
                    SharedString::from(format!("icon-initials-{}", crate::ui::widgets::initials(&bare.name))),
                    Duration::from_millis(320),
                    |el, t| {
                        let k = 1.0 - (1.0 - t).powi(3);
                        el.opacity(0.6 + 0.4 * k)
                    },
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.45))
                    .text_color(gpui_kit::white())
                    .text_size(px(10.4))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .opacity(if busy { 1.0 } else { 0.0 })
                    .id("server-icon-tile-shade")
                    .group_hover("server-icon-tile", |s| s.opacity(1.0))
                    .child(if busy {
                        spinner("icon-busy", 20.0, window)
                    } else {
                        icon("camera").size(px(20.0)).into_any_element()
                    })
                    .child(if busy { String::new() } else { t("workspace.picture.changeShort").to_uppercase() }),
            );
        let fg = p.foreground;
        let destructive = p.destructive;
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(
                button(
                    "server-icon-pick",
                    if value.is_empty() {
                        t("workspace.picture.uploadShort")
                    } else {
                        t("workspace.picture.changeShort")
                    },
                    Some("image-up"),
                    Look::Outline,
                    false,
                    p,
                )
                .rounded(radius_xl())
                .when(busy, |el| el.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.pick_icon(cx))),
            )
            .when(!value.is_empty(), |el| {
                el.child(
                    super::pages::hover_button(
                        "server-icon-remove",
                        t("system.picture.remove"),
                        Some("trash"),
                        false,
                        p,
                        move |s| s.text_color(destructive),
                    )
                    .text_color(p.muted_foreground)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let o = &mut this.pages.overview;
                        o.icon = Some(String::new());
                        o.picture_error = None;
                        o.icon_link.update(cx, |s, cx| s.set_value("", window, cx));
                        cx.notify();
                    })),
                )
            })
            .child(
                div()
                    .id("server-icon-link")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded(radius_lg())
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(fg))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let o = &mut this.pages.overview;
                        o.link_open = !o.link_open;
                        if o.link_open {
                            let now = o.icon.clone();
                            if let Some(v) = now {
                                o.icon_link.update(cx, |s, cx| s.set_value(v, window, cx));
                            }
                        }
                        cx.notify();
                    }))
                    .child(icon("link").size(px(14.0)))
                    .child(if link_open { t("workspace.picture.hideLink") } else { t("workspace.picture.useLink") }),
            );
        if link_open && self.pages.overview.icon_link.read(cx).value().is_empty() && !value.is_empty() {
            let v = value.to_owned();
            self.pages.overview.icon_link.update(cx, |s, cx| s.set_value(v, window, cx));
        }
        let _ = server;
        let error = self.pages.overview.picture_error.clone();
        let link = self.pages.overview.icon_link.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().flex().items_center().gap(px(16.0)).child(tile).child(actions))
            .when(link_open, |el| {
                el.child(motion::rise(
                    div().pt(px(4.0)).child(boxed(
                        Input::new(&link).appearance(false),
                        44.0,
                        focused(&link, window, cx),
                        p,
                    )),
                    "server-icon-link-in",
                    Duration::ZERO,
                    -6.0,
                ))
            })
            .when_some(error, |el, e| {
                el.child(div().text_sm().font_weight(FontWeight::BOLD).text_color(p.destructive).child(e))
            })
            .into_any_element()
    }

    /// Where join messages go, as the web's dropdown of text channels (or none).
    fn join_menu(
        &mut self,
        channels: &[pb::Channel],
        system: &str,
        greeting: Option<&str>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.pages.overview.menu;
        let (hover, open_border) = (alpha(p.primary, 0.4), alpha(p.primary, 0.6));
        let trigger = div()
            .id("join-messages-trigger")
            .h(px(44.0))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(if open { open_border } else { p.border.into() })
            .text_sm()
            .cursor_pointer()
            .hover(move |s| s.border_color(hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.pages.overview.menu = !this.pages.overview.menu;
                cx.notify();
            }))
            .child(icon("hash").size(px(16.0)).text_color(p.muted_foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(greeting.map(str::to_owned).unwrap_or_else(|| t("serversettings.overview.dontPost"))),
            )
            .child(
                icon(if open { "chevron-up" } else { "chevron-down" }).size(px(16.0)).text_color(p.muted_foreground),
            );
        let mut wrap = div().relative().child(trigger);
        if open {
            let mut items: Vec<(String, String)> = vec![(String::new(), t("serversettings.overview.dontPost"))];
            items.extend(channels.iter().map(|c| (c.id.clone(), format!("#{}", c.name))));
            let mut list = div()
                .id("join-messages-menu")
                .absolute()
                .top(px(48.0))
                .left_0()
                .w(px(256.0))
                .max_h(px(288.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(4.0))
                .rounded(crate::ui::theme::radius_md())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .shadow(crate::ui::settings_controls::shadow_lg())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.pages.overview.menu = false;
                    cx.notify();
                }));
            for (n, (id, name)) in items.into_iter().enumerate() {
                let on = id == system;
                let hover = p.accent;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("join-messages-item-{n}")))
                        .relative()
                        .flex()
                        .items_center()
                        .py(px(6.0))
                        .pl(px(32.0))
                        .pr(px(8.0))
                        .rounded(px((f32::from(crate::ui::theme::radius_md()) - 2.0).max(0.0)))
                        .text_sm()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pages.overview.system = Some(id.clone());
                            this.pages.overview.menu = false;
                            cx.notify();
                        }))
                        .when(on, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .left(px(8.0))
                                    .top_0()
                                    .bottom_0()
                                    .w(px(14.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().size(px(8.0)).rounded_full().bg(p.foreground)),
                            )
                        })
                        .child(div().truncate().child(name)),
                );
            }
            wrap = wrap.child(
                gpui_kit::deferred(motion::rise(list, "join-messages-menu-in", Duration::ZERO, -4.0)).with_priority(1),
            );
        }
        wrap.into_any_element()
    }

    // ───────────────────────── Access ─────────────────────────

    fn discard_access(&mut self, cx: &mut Context<Self>) {
        let o = &mut self.pages.overview;
        o.access = AccessPatch::default();
        o.access_error = None;
        cx.notify();
    }

    fn save_access(&mut self, patch: AccessPatch, cx: &mut Context<Self>) {
        self.pages.overview.access_saving = true;
        self.pages.overview.access_error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.update_access(&key, &sid, patch).await }, |this, result, cx| {
            let o = &mut this.pages.overview;
            o.access_saving = false;
            match result {
                Ok(_) => o.access = AccessPatch::default(),
                Err(err) => o.access_error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn access_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let linked_offered = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.node.as_ref())
                .and_then(|n| n.auth.as_ref())
                .is_some_and(|a| a.linked_sign_in)
        });
        let a = self.pages.overview.access.clone();
        let discoverable = a.discoverable.unwrap_or(server.discoverable);
        let applications = a.applications.unwrap_or(server.applications);
        let linked_only = a.linked_only.unwrap_or(server.linked_only);
        let min_age = a.min_account_age_seconds.unwrap_or(server.min_account_age_seconds);
        // Only what differs from the server counts (picking the saved value again isn't a change).
        let patch = AccessPatch {
            discoverable: (discoverable != server.discoverable).then_some(discoverable),
            applications: (applications != server.applications).then_some(applications),
            linked_only: (linked_only != server.linked_only).then_some(linked_only),
            min_account_age_seconds: (min_age != server.min_account_age_seconds).then_some(min_age),
        };
        let changes = [
            patch.discoverable.is_some(),
            patch.applications.is_some(),
            patch.linked_only.is_some(),
            patch.min_account_age_seconds.is_some(),
        ]
        .into_iter()
        .filter(|c| *c)
        .count();
        if changes > 0 {
            self.bar = Some(bar_with_error(
                "access-bar",
                changes,
                self.pages.overview.access_saving,
                self.pages.overview.access_error.as_deref(),
                p,
                cx,
                |this, _, cx| this.discard_access(cx),
                move |this, _, cx| this.save_access(patch.clone(), cx),
            ));
        }
        let form_w = if self.wide { self.column - 288.0 - 40.0 } else { self.column };

        let who = part(true, false, 12.0, p)
            .child(heading(&t("serversettings.access.whoCanJoin"), Some(&t("serversettings.access.whoCanJoinHint")), p))
            .child(choice(
                "discoverable",
                Some(usize::from(discoverable)),
                vec![
                    Opt::new(t("serversettings.access.inviteOnly"), t("serversettings.access.inviteOnlyHint"), "lock"),
                    Opt::new(t("serversettings.access.anyone"), t("serversettings.access.anyoneHint"), "compass"),
                ],
                form_w,
                p,
                window,
                cx,
                |this: &mut Self, i, cx| {
                    this.pages.overview.access.discoverable = Some(i == 1);
                    cx.notify();
                },
            ));
        let how = part(false, false, 12.0, p)
            .child(heading(&t("serversettings.access.howIn"), Some(&t("serversettings.access.howInHint")), p))
            .child(choice(
                "applications",
                Some(usize::from(applications)),
                vec![
                    Opt::new(t("serversettings.access.joinNow"), t("serversettings.access.joinNowHint"), "door-open"),
                    Opt::new(
                        t("serversettings.nav.applyToJoin"),
                        t("serversettings.access.applyHint"),
                        "clipboard-pen",
                    ),
                ],
                form_w,
                p,
                window,
                cx,
                |this: &mut Self, i, cx| {
                    this.pages.overview.access.applications = Some(i == 1);
                    cx.notify();
                },
            ))
            .when(server.applications && !applications, |el| {
                el.child(super::pages::slide_in(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(amber(p))
                        .child(t("serversettings.access.dropped")),
                    "access-dropped",
                ))
            });
        let linked = part(false, false, 8.0, p).child(toggle(
            "linked-only",
            &t("serversettings.nav.linkedOnly"),
            Some(&if linked_offered || linked_only {
                t("serversettings.access.linkedOnlyHint")
            } else {
                t("serversettings.access.linkedUnavailable")
            }),
            linked_only,
            !linked_offered && !linked_only,
            p,
            window,
            cx,
            |this: &mut Self, on, cx| {
                this.pages.overview.access.linked_only = Some(on);
                cx.notify();
            },
        ));
        let mut ages: Vec<(i32, String)> = ACCOUNT_AGES.iter().map(|(v, k)| (*v, t(k))).collect();
        if !ages.iter().any(|(v, _)| *v == server.min_account_age_seconds) {
            ages.push((
                server.min_account_age_seconds,
                super::pages::duration(i64::from(server.min_account_age_seconds)),
            ));
            ages.sort_by_key(|(v, _)| *v);
        }
        let chosen = ages.iter().position(|(v, _)| *v == min_age).unwrap_or(0);
        let values: Vec<i32> = ages.iter().map(|(v, _)| *v).collect();
        let age = part(false, true, 12.0, p)
            .child(heading(&t("serversettings.nav.accountAge"), Some(&t("serversettings.access.accountAgeHint")), p))
            .child(chips(
                "account-age",
                ages.into_iter().map(|(_, l)| l).collect(),
                chosen,
                p,
                cx,
                move |this: &mut Self, i, cx| {
                    this.pages.overview.access.min_account_age_seconds = values.get(i).copied();
                    cx.notify();
                },
            ));
        let form = div()
            .flex()
            .flex_col()
            .child(self.mark("discoverable", who, p))
            .child(self.mark("applications", how, p))
            .child(self.mark("linked-only", linked, p))
            .child(self.mark("account-age", age, p));
        let mut shown = server.clone();
        shown.discoverable = discoverable;
        shown.applications = applications;
        shown.linked_only = linked_only;
        shown.min_account_age_seconds = min_age;
        let preview = div().flex().flex_col().gap(px(16.0)).child(browse_card(&shown, p)).child(gate_preview(
            min_age,
            applications,
            linked_only,
            p,
        ));
        crate::ui::settings_controls::with_preview(form, preview, self.wide, p)
    }
}

/// The server as people find it in Browse, or hidden from it (the web's `BrowseCard`).
fn browse_card(server: &pb::Server, p: &Palette) -> AnyElement {
    let members = server.member_count;
    let hidden = !server.discoverable;
    let body = div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(20.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(server_icon(server, 56.0, 56.0 * 0.32, p).text_size(px(18.0)))
                .child(
                    div()
                        .min_w_0()
                        .child(
                            div()
                                .text_lg()
                                .line_height(px(28.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .truncate()
                                .child(swapping("browse-name", server.name.clone(), 18.0)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_xs()
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(icon("users").size(px(14.0)))
                                .child(counted(
                                    "browse-members",
                                    "serversettings.shared.members",
                                    members.max(0) as u64,
                                    12.0,
                                )),
                        ),
                ),
        )
        .child(div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).line_clamp(4).child(
            if server.description.trim().is_empty() {
                t("serversettings.browse.noDescription")
            } else {
                server.description.clone()
            },
        ))
        .children(crate::ui::instance_home::server_door(server, p))
        .child(
            div()
                .h(px(36.0))
                .rounded(radius_xl())
                .bg(p.primary)
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(p.primary_foreground)
                .child(swapping(
                    "browse-join",
                    if server.applications {
                        t("serversettings.nav.applyToJoin")
                    } else {
                        t("serversettings.browse.join")
                    },
                    14.0,
                )),
        );
    let faded = motion::once(
        div().child(body),
        SharedString::from(format!("browse-fade-{hidden}")),
        Duration::from_millis(300),
        move |el, t| {
            let (from, to) = if hidden { (1.0, 0.2) } else { (0.2, 1.0) };
            el.opacity(from + (to - from) * t)
        },
    );
    preview_card(p)
        .relative()
        .overflow_hidden()
        .child(faded)
        .when(hidden, |el| {
            el.child(
                div().absolute().inset_0().flex().items_center().justify_center().p(px(24.0)).child(motion::rise(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .child(icon("eye-off").size(px(24.0)).text_color(p.muted_foreground))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(t("serversettings.browse.hidden")),
                        )
                        .child(
                            div().text_xs().text_color(p.muted_foreground).child(t("serversettings.browse.hiddenHint")),
                        ),
                    "browse-hidden",
                    Duration::ZERO,
                    6.0,
                )),
            )
        })
        .into_any_element()
}

/// A join message as it will look, or a note that there won't be one (the web's `JoinPreview`).
fn join_preview(channel: Option<&str>, me: Option<&pb::User>, p: &Palette) -> AnyElement {
    const LINES: [&str; 8] = [
        "chat.join.line1",
        "chat.join.line2",
        "chat.join.line3",
        "chat.join.line4",
        "chat.join.line5",
        "chat.join.line6",
        "chat.join.line7",
        "chat.join.line8",
    ];
    let id = me.map(|u| u.id.as_str()).unwrap_or_default();
    let name = me.map(user_name).unwrap_or_default();
    let line = t_with(LINES[(crate::ui::widgets::hue_of(id) as usize) % LINES.len()], &[("name", Arg::Str(&name))]);
    let on = channel.is_some();
    let arrow = div().text_color(gpui_kit::rgb(0x10b981)).child("→");
    let arrow: AnyElement = if on {
        // The web nudges it forever; a cached page plays its flourishes once.
        motion::once(div().child(arrow), "join-arrow", Duration::from_millis(1600), |el, t| {
            el.translate_x(px(4.0 * (t * std::f32::consts::PI).sin()))
        })
    } else {
        arrow.into_any_element()
    };
    preview_card(p)
        .overflow_hidden()
        .p(px(16.0))
        .child(
            div()
                .mb(px(8.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(icon("hash").size(px(14.0)))
                .child(motion::rise(
                    div().child(channel.map(str::to_owned).unwrap_or_else(|| t("serversettings.overview.noChannel"))),
                    SharedString::from(format!("join-channel-{}", channel.unwrap_or("-"))),
                    Duration::ZERO,
                    10.0,
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_sm()
                .line_height(px(20.0))
                .opacity(if on { 1.0 } else { 0.35 })
                .child(arrow)
                .child(avatar(me, 24.0, p))
                .child(div().min_w_0().truncate().child(line)),
        )
        .into_any_element()
}

/// Three accounts at the door, and what happens to each (the web's `GatePreview`).
fn gate_preview(min_age: i32, applications: bool, linked_only: bool, p: &Palette) -> AnyElement {
    let people = [
        (t("serversettings.access.gate.hoursOld"), t("serversettings.access.gate.linked"), 2 * 3600, true, 330.0),
        (t("serversettings.access.gate.monthOld"), t("serversettings.access.gate.linked"), 30 * 86_400, true, 200.0),
        (t("serversettings.access.gate.monthOld"), t("serversettings.access.gate.local"), 30 * 86_400, false, 140.0),
    ];
    let emerald = if p.dark { gpui_kit::rgb(0x34d399) } else { gpui_kit::rgb(0x059669) };
    let amber = amber(p);
    let mut card = preview_card(p).flex().flex_col().gap(px(10.0)).p(px(16.0)).child(
        div()
            .text_xs()
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.muted_foreground)
            .child(t("serversettings.access.gate.title")),
    );
    for (n, (name, sub, age, linked, hue)) in people.into_iter().enumerate() {
        let wait = i64::from(min_age) - age;
        let out = linked_only && !linked;
        let (state, glyph, text, fg, bg): (String, &str, String, Hsla, Hsla) = if out {
            (
                "out".into(),
                "x",
                t("serversettings.access.gate.cantJoin"),
                p.destructive.into(),
                alpha(p.destructive, 0.15),
            )
        } else if wait > 0 {
            (
                format!("wait-{wait}"),
                "hourglass",
                t_with(
                    "serversettings.access.gate.waits",
                    &[("time", Arg::Str(&super::pages::time_left(wait * 1000)))],
                ),
                amber,
                amber.opacity(0.15),
            )
        } else if applications {
            (
                "apply".into(),
                "clipboard-pen",
                t("serversettings.access.gate.applies"),
                p.primary.into(),
                alpha(p.primary, 0.15),
            )
        } else {
            (
                "in".into(),
                "check",
                t("serversettings.access.gate.joins"),
                emerald.into(),
                Hsla::from(emerald).opacity(0.15),
            )
        };
        let from = gpui_kit::hsla(hue / 360.0, 0.62, 0.68, 1.0);
        let to = gpui_kit::hsla(((hue + 40.0) % 360.0) / 360.0, 0.6, 0.5, 1.0);
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_sm()
                .child(div().size(px(28.0)).flex_none().rounded_full().bg(gpui_kit::linear_gradient(
                    135.0,
                    gpui_kit::linear_color_stop(from, 0.0),
                    gpui_kit::linear_color_stop(to, 1.0),
                )))
                .child(
                    div().min_w_0().flex_1().child(div().truncate().line_height(px(17.5)).child(name)).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(11.2))
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .when(linked, |el| el.child(icon("badge-check").size(px(12.0))))
                            .child(sub),
                    ),
                )
                .child(motion::once(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .rounded_full()
                        .px(px(8.0))
                        .py(px(2.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .bg(bg)
                        .text_color(fg)
                        .child(icon(glyph).size(px(12.0)))
                        .child(text),
                    SharedString::from(format!("gate-{n}-{state}")),
                    Duration::from_millis(300),
                    |el, t| {
                        let k = 1.0 - (1.0 - t).powi(3);
                        el.opacity(k)
                    },
                )),
        );
    }
    card.into_any_element()
}
