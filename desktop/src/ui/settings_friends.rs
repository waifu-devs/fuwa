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
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{Badge, Opt, caps, choice, switch, toggle};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_xl};
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
    pub(crate) fn friends_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        let (status, settings) = self.core.shared.read(|s| {
            s.instance(&key).map(|i| (i.friends.status, i.friends.settings)).unwrap_or((FriendsStatus::Off, None))
        });
        if status == FriendsStatus::Unsupported {
            return div()
                .text_sm()
                .text_color(p.muted_foreground)
                .child(t("accountsettings.friends.unsupported"))
                .into_any_element();
        }
        let current = settings.unwrap_or_default();
        let w = self.column;
        let opts = |list: [Choice; 3]| {
            list.iter().map(|(_, glyph, label, hint)| Opt::new(label.clone(), hint.clone(), glyph)).collect::<Vec<_>>()
        };
        let req = requests();
        let at = req.iter().position(|(v, ..)| *v == current.requests_from);
        let values: Vec<i32> = req.iter().map(|(v, ..)| *v).collect();
        let k = key.clone();
        let requests_choice = choice("friend-requests", at, opts(req), w, p, window, cx, move |this, n, cx| {
            let v = values[n];
            this.save_friends(&k, cx, |s| s.requests_from = v)
        });
        let msg = messages();
        let at = msg.iter().position(|(v, ..)| *v == current.direct_messages_from);
        let values: Vec<i32> = msg.iter().map(|(v, ..)| *v).collect();
        let k = key.clone();
        let messages_choice = choice("direct-messages", at, opts(msg), w, p, window, cx, move |this, n, cx| {
            let v = values[n];
            this.save_friends(&k, cx, |s| s.direct_messages_from = v)
        });
        let (k1, k2) = (key.clone(), key.clone());
        let see = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(toggle(
                "friends-online",
                &t("accountsettings.friends.showOnline"),
                Some(&t("accountsettings.friends.showOnlineHint")),
                !current.hide_online,
                false,
                p,
                window,
                cx,
                move |this, on, cx| this.save_friends(&k1, cx, |s| s.hide_online = !on),
            ))
            .child(toggle(
                "friends-mutual",
                &t("accountsettings.friends.showMutual"),
                Some(&t("accountsettings.friends.showMutualHint")),
                !current.hide_mutual_friends,
                false,
                p,
                window,
                cx,
                move |this, on, cx| this.save_friends(&k2, cx, |s| s.hide_mutual_friends = !on),
            ));
        let reset = |key: &str, f: fn(&mut pb::FriendSettings)| {
            let key = key.to_owned();
            std::rc::Rc::new(move |this: &mut SettingsView, cx: &mut Context<SettingsView>| {
                this.save_friends(&key, cx, f)
            }) as std::rc::Rc<dyn Fn(&mut SettingsView, &mut Context<SettingsView>)>
        };
        let list = self.stack(
            [
                (
                    "friend-requests",
                    t("settings.nav.friendRequests"),
                    Some(t("accountsettings.friends.requestsHint")),
                    Badge::Custom {
                        changed: current.requests_from != pb::FriendRequestsFrom::Unspecified as i32,
                        reset: reset(&key, |s| s.requests_from = pb::FriendRequestsFrom::Unspecified as i32),
                    },
                    requests_choice,
                ),
                (
                    "direct-messages",
                    t("settings.nav.directMessages"),
                    Some(t("accountsettings.friends.messagesHint")),
                    Badge::Custom {
                        changed: current.direct_messages_from != pb::DirectMessagesFrom::Unspecified as i32,
                        reset: reset(&key, |s| s.direct_messages_from = pb::DirectMessagesFrom::Unspecified as i32),
                    },
                    messages_choice,
                ),
                (
                    "friends-see",
                    t("settings.nav.friendsSee"),
                    None,
                    Badge::Custom {
                        changed: current.hide_online || current.hide_mutual_friends,
                        reset: reset(&key, |s| {
                            s.hide_online = false;
                            s.hide_mutual_friends = false;
                        }),
                    },
                    see.into_any_element(),
                ),
            ],
            p,
            cx,
        );
        div()
            .flex()
            .flex_col()
            .child(list)
            .when_some(error_line(self.friends_error.as_deref(), p), |el, line| el.child(line))
            .into_any_element()
    }

    /// "Show what I'm doing": whether people who share a server with you see
    /// the games and apps this app picks up, and in which servers. Off until
    /// you turn it on; your status shows either way (the web's `ActivitySharing`).
    pub(crate) fn activity_section(
        &mut self,
        key: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (settings, allowed, servers, name) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let allowed = i.node.as_ref().is_some_and(|n| n.rich_presence);
            Some((i.presence.clone()?, allowed, i.servers.clone(), i.name()))
        })?;
        let on = settings.show_activity && allowed;
        let k = key.to_owned();
        let mut card = div().rounded(radius_3xl()).border_1().border_color(p.border).bg(p.card).p(px(20.0)).child(
            div()
                .flex()
                .items_start()
                .gap(px(16.0))
                .child(
                    div()
                        .size(px(48.0))
                        .flex_none()
                        .rounded(radius_2xl())
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("gamepad").size(px(24.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("settings.nav.activity")))
                        .child(div().text_sm().text_color(p.muted_foreground).child(if allowed {
                            t("accountsettings.privacy.activityHint")
                        } else {
                            t_with("accountsettings.privacy.activityOff", &[("instance", Arg::Str(&name))])
                        })),
                )
                .child(div().mt(px(4.0)).child(switch(
                    "activity-share",
                    on,
                    !allowed,
                    p,
                    window,
                    cx,
                    move |this, on, cx| this.save_presence(&k, on, None, cx),
                ))),
        );
        if on && !servers.is_empty() {
            let mut list = div().flex().flex_col().gap(px(4.0));
            for server in &servers {
                let shared = !settings.hidden_server_ids.contains(&server.id);
                let (k, id) = (key.to_owned(), server.id.clone());
                let (k2, id2) = (k.clone(), id.clone());
                let hover = alpha(p.muted, 0.7);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("share-in-{}", server.id)))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(radius_xl())
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.save_presence(&k2, !shared, Some(id2.clone()), cx)),
                        )
                        .child(crate::ui::widgets::server_icon(server, 32.0, 12.0, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(server.name.clone()),
                        )
                        .child(switch(
                            SharedString::from(format!("activity-in-{}", server.id)),
                            shared,
                            false,
                            p,
                            window,
                            cx,
                            move |this, on, cx| this.save_presence(&k, on, Some(id.clone()), cx),
                        )),
                );
            }
            card = card.child(motion::rise(
                div()
                    .child(
                        caps(&t("accountsettings.privacy.shareHere"), p)
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .mt(px(20.0))
                            .mb(px(8.0)),
                    )
                    .child(list),
                "activity-servers",
                std::time::Duration::ZERO,
                -8.0,
            ));
        }
        Some(self.found_mark("activity-sharing", div().child(card), p))
    }

    /// Turns sharing on or off, everywhere or in one server. The page shows it
    /// at once, and reads the instance's word again if saving fails.
    pub(crate) fn save_presence(&mut self, key: &str, on: bool, server: Option<String>, cx: &mut Context<Self>) {
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
