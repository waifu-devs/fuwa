//! Settings > Friends and privacy: whether people see what you're doing,
//! who may ask you to be friends or start a conversation with you, and what
//! your friends see. Kept on the instance,
//! so it holds on every device; each change saves at once. Like the web
//! app's `settings/account/FriendPrivacy.tsx`.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::friends::FriendsStatus;
use crate::core::i18n::{Arg, t, t_with};
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::{SettingsView, toggle_row};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{error_line, icon};

type Choice = (i32, &'static str, String, String);

fn requests() -> [Choice; 3] {
    [
        (
            pb::FriendRequestsFrom::Unspecified as i32,
            "earth",
            t("accountsettings.friends.everyone"),
            t("accountsettings.friends.everyoneHint"),
        ),
        (
            pb::FriendRequestsFrom::SharedServers as i32,
            "server",
            t("accountsettings.friends.serverFriends"),
            t("accountsettings.friends.serverFriendsHint"),
        ),
        (
            pb::FriendRequestsFrom::Nobody as i32,
            "ban",
            t("accountsettings.friends.nobody"),
            t("accountsettings.friends.nobodyHint"),
        ),
    ]
}

fn messages() -> [Choice; 3] {
    [
        (
            pb::DirectMessagesFrom::Unspecified as i32,
            "users",
            t("accountsettings.friends.everyone"),
            t("accountsettings.friends.messagesEveryoneHint"),
        ),
        (
            pb::DirectMessagesFrom::Friends as i32,
            "heart",
            t("accountsettings.friends.friendsOnly"),
            t("accountsettings.friends.friendsOnlyHint"),
        ),
        (
            pb::DirectMessagesFrom::Nobody as i32,
            "message-circle-off",
            t("accountsettings.friends.nobodyNew"),
            t("accountsettings.friends.nobodyNewHint"),
        ),
    ]
}

impl SettingsView {
    pub(crate) fn friends_page(&mut self, p: &Palette, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else {
            return div()
                .p(px(18.0))
                .rounded(corner(16.0))
                .bg(p.secondary)
                .text_color(p.muted_foreground)
                .child(t("desktop.friends.signInFirst"))
                .into_any_element();
        };
        let (status, settings) = self.core.shared.read(|s| {
            s.instance(&key).map(|i| (i.friends.status, i.friends.settings)).unwrap_or((FriendsStatus::Off, None))
        });
        let mut body = div().flex().flex_col().gap(px(18.0));
        if let Some(picker) = self.account_picker(&key, p, cx) {
            body = body.child(picker);
        }
        if let Some(sharing) = self.activity_section(&key, p, cx) {
            body = body.child(sharing);
        }
        if status == FriendsStatus::Unsupported {
            return body
                .child(
                    div()
                        .p(px(18.0))
                        .rounded(corner(16.0))
                        .bg(p.secondary)
                        .text_color(p.muted_foreground)
                        .child(t("accountsettings.friends.unsupported")),
                )
                .into_any_element();
        }
        let current = settings.unwrap_or_default();
        let loaded = status == FriendsStatus::Ready;

        body = body.child(motion::rise(
            section(
                &t("settings.nav.friendRequests"),
                &t("accountsettings.friends.requestsHint"),
                choices("friend-requests", &requests(), current.requests_from, loaded, &key, p, cx, |s, v| {
                    s.requests_from = v
                }),
                p,
            ),
            "friends-requests",
            std::time::Duration::ZERO,
            8.0,
        ));
        body = body.child(motion::rise(
            section(
                &t("settings.nav.directMessages"),
                &t("accountsettings.friends.messagesHint"),
                choices("friend-dms", &messages(), current.direct_messages_from, loaded, &key, p, cx, |s, v| {
                    s.direct_messages_from = v
                }),
                p,
            ),
            "friends-dms",
            std::time::Duration::from_millis(40),
            8.0,
        ));
        let (k1, k2) = (key.clone(), key.clone());
        body = body.child(motion::rise(
            section(
                &t("settings.nav.friendsSee"),
                "",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(toggle_row(
                        "friends-online",
                        &t("accountsettings.friends.showOnline"),
                        &t("accountsettings.friends.showOnlineHint"),
                        !current.hide_online,
                        p,
                        cx,
                        move |this, on, cx| this.save_friends(&k1, cx, |s| s.hide_online = !on),
                    ))
                    .child(toggle_row(
                        "friends-mutual",
                        &t("accountsettings.friends.showMutual"),
                        &t("accountsettings.friends.showMutualHint"),
                        !current.hide_mutual_friends,
                        p,
                        cx,
                        move |this, on, cx| this.save_friends(&k2, cx, |s| s.hide_mutual_friends = !on),
                    ))
                    .into_any_element(),
                p,
            ),
            "friends-see",
            std::time::Duration::from_millis(80),
            8.0,
        ));
        body.when_some(error_line(self.friends_error.as_deref(), p), |el, line| el.child(line))
            .child(div().text_sm().text_color(p.muted_foreground).child(t("desktop.friends.blockingNote")))
            .into_any_element()
    }

    /// "Show what I'm doing": whether people who share a server with you see
    /// the games and apps this app picks up, and in which servers. Off until
    /// you turn it on; your status shows either way. Like the web's
    /// `ActivitySharing`.
    fn activity_section(&mut self, key: &str, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (settings, allowed, servers, name) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let allowed = i.node.as_ref().is_some_and(|n| n.rich_presence);
            Some((i.presence.clone()?, allowed, i.servers.clone(), i.name()))
        })?;
        let on = settings.show_activity && allowed;
        let k = key.to_owned();
        let mut content = div().flex().flex_col().gap(px(10.0));
        content = if allowed {
            content.child(toggle_row(
                "activity-share",
                &t("settings.nav.activity"),
                &t("desktop.friends.activityHint"),
                on,
                p,
                cx,
                move |this, on, cx| this.save_presence(&k, on, None, cx),
            ))
        } else {
            content.child(
                div()
                    .p(px(16.0))
                    .rounded(corner(16.0))
                    .bg(p.secondary)
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t_with("accountsettings.privacy.activityOff", &[("instance", Arg::Str(&name))])),
            )
        };
        if on && !servers.is_empty() {
            let mut list = div().flex().flex_col().gap(px(2.0)).child(
                div()
                    .pt(px(6.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(t("accountsettings.privacy.shareHere").to_uppercase()),
            );
            for server in &servers {
                let shared = !settings.hidden_server_ids.contains(&server.id);
                let (k, id) = (key.to_owned(), server.id.clone());
                let entity = cx.entity().downgrade();
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(corner(12.0))
                        .child(crate::ui::widgets::server_icon(server, 32.0, 10.0, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(server.name.clone()),
                        )
                        .child(
                            gpui_kit::component::switch::Switch::new(SharedString::from(format!(
                                "activity-in-{}",
                                server.id
                            )))
                            .checked(shared)
                            .on_change(move |checked, _, cx| {
                                let (k, id, checked) = (k.clone(), id.clone(), *checked);
                                let _ = entity.update(cx, |this, cx| this.save_presence(&k, checked, Some(id), cx));
                            }),
                        ),
                );
            }
            content = content.child(motion::rise(list, "activity-servers", std::time::Duration::ZERO, 6.0));
        }
        Some(
            motion::rise(
                section(&t("desktop.friends.whatYoureDoing"), "", content.into_any_element(), p),
                "friends-activity",
                std::time::Duration::ZERO,
                8.0,
            )
            .into_any_element(),
        )
    }

    /// Turns sharing on or off, everywhere or in one server. The page shows it
    /// at once, and reads the instance's word again if saving fails.
    fn save_presence(&mut self, key: &str, on: bool, server: Option<String>, cx: &mut Context<Self>) {
        self.core.shared.instance(key, |i| {
            if let Some(settings) = i.presence.as_mut() {
                match &server {
                    Some(id) => crate::core::account::share_in(settings, id, on),
                    None => settings.show_activity = on,
                }
            }
        });
        let (core, key) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move {
            let result = match &server {
                Some(id) => core.share_activity_in(&key, id, on).await,
                None => core.share_activity(&key, on).await,
            };
            if result.is_err() {
                core.refresh_presence(&key).await;
            }
            result
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.friends_error = result
                    .err()
                    .map(|e| t_with("accountsettings.privacy.saveFailed", &[("error", Arg::Str(&e.message))]));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Changes one setting and saves them all; the page shows it at once and puts it back if saving fails.
    fn save_friends(&mut self, key: &str, cx: &mut Context<Self>, change: impl FnOnce(&mut pb::FriendSettings)) {
        let mut settings =
            self.core.shared.read(|s| s.instance(key).and_then(|i| i.friends.settings)).unwrap_or_default();
        change(&mut settings);
        let (core, key) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.save_friend_settings(&key, settings).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.friends_error = result.err().map(|e| e.message);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn section(title: &str, hint: &str, content: AnyElement, p: &Palette) -> impl IntoElement + gpui_kit::Styled + use<> {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
        .when(!hint.is_empty(), |el| el.child(div().text_sm().text_color(p.muted_foreground).child(hint.to_owned())))
        .child(content)
}

/// Three cards side by side; the chosen one is outlined and ticked.
#[allow(clippy::too_many_arguments)]
fn choices(
    id: &'static str,
    options: &[Choice],
    value: i32,
    loaded: bool,
    key: &str,
    p: &Palette,
    cx: &mut Context<SettingsView>,
    set: fn(&mut pb::FriendSettings, i32),
) -> AnyElement {
    div()
        .flex()
        .gap(px(8.0))
        .children(options.iter().enumerate().map(|(n, (v, glyph, label, hint))| {
            let (v, glyph) = (*v, *glyph);
            let on = v == value;
            let key = key.to_owned();
            div()
                .id(SharedString::from(format!("{id}-{n}")))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(12.0))
                .rounded(corner(14.0))
                .border_1()
                .border_color(if on { p.primary } else { p.border })
                .bg(if on { alpha(p.primary, 0.1) } else { p.card.into() })
                .when(!loaded, |el| el.opacity(0.6))
                .cursor_pointer()
                .hover({
                    let c = alpha(p.primary, if on { 0.14 } else { 0.05 });
                    move |s| s.bg(c)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !on {
                        this.save_friends(&key, cx, |s| set(s, v));
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(icon(glyph).size(px(16.0)).text_color(if on { p.primary } else { p.muted_foreground }))
                        .child(div().flex_1().font_weight(FontWeight::BOLD).text_sm().child(label.clone()))
                        .when(on, |el| {
                            el.child(motion::once(
                                icon("check").size(px(14.0)).text_color(p.primary),
                                SharedString::from(format!("{id}-tick-{v}")),
                                std::time::Duration::from_millis(260),
                                |el, t| el.size(px(14.0 * (0.4 + 0.6 * t))),
                            ))
                        }),
                )
                .child(div().text_xs().text_color(p.muted_foreground).child(hint.clone()))
        }))
        .into_any_element()
}
