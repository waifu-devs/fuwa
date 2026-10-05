//! Settings > Friends and privacy: who may ask you to be friends or start a
//! conversation with you, and what your friends see. Kept on the instance,
//! so it holds on every device; each change saves at once. Like the web
//! app's `settings/account/FriendPrivacy.tsx`.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::friends::FriendsStatus;
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::{SettingsView, toggle_row};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{error_line, icon};

type Choice = (i32, &'static str, &'static str, &'static str);

const REQUESTS: [Choice; 3] = [
    (pb::FriendRequestsFrom::Unspecified as i32, "earth", "Everyone", "Anyone on this instance"),
    (pb::FriendRequestsFrom::SharedServers as i32, "server", "Server friends", "People in a server with you"),
    (pb::FriendRequestsFrom::Nobody as i32, "ban", "Nobody", "You can still send them"),
];

const MESSAGES: [Choice; 3] = [
    (pb::DirectMessagesFrom::Unspecified as i32, "users", "Everyone", "Friends and people in a server with you"),
    (pb::DirectMessagesFrom::Friends as i32, "heart", "Friends only", "Only friends can start one"),
    (pb::DirectMessagesFrom::Nobody as i32, "message-circle-off", "Nobody new", "Conversations you have keep going"),
];

impl SettingsView {
    pub(crate) fn friends_page(&mut self, p: &Palette, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else {
            return div()
                .p(px(18.0))
                .rounded(corner(16.0))
                .bg(p.secondary)
                .text_color(p.muted_foreground)
                .child("Sign in to an instance first. Your friends live there.")
                .into_any_element();
        };
        let (status, settings) = self.core.shared.read(|s| {
            s.instance(&key).map(|i| (i.friends.status, i.friends.settings)).unwrap_or((FriendsStatus::Off, None))
        });
        let mut body = div().flex().flex_col().gap(px(18.0));
        if let Some(picker) = self.account_picker(&key, p, cx) {
            body = body.child(picker);
        }
        if status == FriendsStatus::Unsupported {
            return body
                .child(
                    div()
                        .p(px(18.0))
                        .rounded(corner(16.0))
                        .bg(p.secondary)
                        .text_color(p.muted_foreground)
                        .child("This instance runs a version of fuwa from before friends."),
                )
                .into_any_element();
        }
        let current = settings.unwrap_or_default();
        let loaded = status == FriendsStatus::Ready;

        body = body.child(motion::rise(
            section(
                "Who can send you friend requests",
                "Requests from anyone else never reach you; they're told you aren't taking them.",
                choices("friend-requests", &REQUESTS, current.requests_from, loaded, &key, p, cx, |s, v| {
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
                "Who can start a conversation with you",
                "Direct messages stay end-to-end encrypted whoever sends them.",
                choices("friend-dms", &MESSAGES, current.direct_messages_from, loaded, &key, p, cx, |s, v| {
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
                "What your friends see",
                "",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(toggle_row(
                        "friends-online",
                        "Show when I'm online",
                        "Friends see a green dot while you have fuwa open. Nobody else ever does.",
                        !current.hide_online,
                        p,
                        cx,
                        move |this, on, cx| this.save_friends(&k1, cx, |s| s.hide_online = !on),
                    ))
                    .child(toggle_row(
                        "friends-mutual",
                        "Show mutual friends",
                        "On profiles, only when you, they and the friend you share all allow it.",
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
            .child(
                div()
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child("Blocking someone, from their profile or your friends list, stops them whatever these say."),
            )
            .into_any_element()
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
        .children(options.iter().enumerate().map(|(n, &(v, glyph, label, hint))| {
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
                        .child(div().flex_1().font_weight(FontWeight::BOLD).text_sm().child(label))
                        .when(on, |el| {
                            el.child(motion::once(
                                icon("check").size(px(14.0)).text_color(p.primary),
                                SharedString::from(format!("{id}-tick-{v}")),
                                std::time::Duration::from_millis(260),
                                |el, t| el.size(px(14.0 * (0.4 + 0.6 * t))),
                            ))
                        }),
                )
                .child(div().text_xs().text_color(p.muted_foreground).child(hint))
        }))
        .into_any_element()
}
