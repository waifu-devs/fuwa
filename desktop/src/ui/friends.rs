//! Friends under Home (docs/friends.md): tabs for who's online, everyone,
//! requests and blocks, a box to ask someone by username, and the buttons on
//! a profile card. Like the web app's `pages/FriendsPage.tsx` and
//! `components/friends/FriendActions.tsx`.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::core::dms::now_ms;
use crate::core::friends::{self, BLOCKED, FRIEND, FriendsStatus, INCOMING, OUTGOING, Tab};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::context_menu::MenuOf;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner, mix};
use crate::ui::widgets::{avatar, badge, icon, icon_button, icon_button_in, pal, primary_button, soft_button};

/// What the Friends screen and the profile card's friend buttons hold.
pub struct Friends {
    pub tab: Tab,
    /// The instance the screen was last opened on, to pick its first tab once.
    opened: Option<String>,
    pub search: Entity<InputState>,
    /// The username box, while "Add friend" is open.
    pub add: Entity<InputState>,
    pub adding: bool,
    sending: bool,
    /// What came of the last request sent from the box: whether it went, and what to say.
    result: Option<(bool, String)>,
    /// People with something on its way, so a button isn't pressed twice.
    busy: HashSet<String>,
    /// Where you stand with the person whose profile is open, as the instance said.
    pub relation: Option<(String, pb::GetRelationshipResponse)>,
    /// Empty the username box on the next draw (a request went).
    clear_add: bool,
}

/// Something done about someone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Request,
    Accept,
    Remove,
    Block,
    Unblock,
}

impl Friends {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let add = cx.new(|cx| InputState::new(window, cx).placeholder("username"));
        let subs = vec![
            cx.subscribe_in(&search, window, |_: &mut FuwaApp, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&add, window, |this: &mut FuwaApp, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.send_typed_request(window, cx),
                InputEvent::Change => {
                    this.friends.result = None;
                    cx.notify();
                }
                _ => {}
            }),
        ];
        (
            Friends {
                tab: Tab::Online,
                opened: None,
                search,
                add,
                adding: false,
                sending: false,
                result: None,
                busy: HashSet::new(),
                relation: None,
                clear_add: false,
            },
            subs,
        )
    }
}

impl FuwaApp {
    /// Does something about someone, then says how it went.
    pub(crate) fn friend_act(&mut self, key: &str, user_id: &str, act: Act, cx: &mut Context<Self>) {
        if !self.friends.busy.insert(user_id.to_owned()) {
            return;
        }
        let name = self
            .core
            .shared
            .read(|s| s.instance(key).and_then(|i| i.users.get(user_id)).map(user_name))
            .unwrap_or_else(|| "them".into());
        let (core, k, id) = (self.core.clone(), key.to_owned(), user_id.to_owned());
        let future = async move {
            match act {
                Act::Request => core.send_friend_request(&k, &id, "").await.map(|f| f.state),
                Act::Accept => core.accept_friend(&k, &id).await.map(|()| FRIEND),
                Act::Remove => core.remove_friend(&k, &id).await.map(|()| 0),
                Act::Block => core.block_user(&k, &id).await.map(|()| BLOCKED),
                Act::Unblock => core.unblock_user(&k, &id).await.map(|()| 0),
            }
        };
        let (k, id) = (key.to_owned(), user_id.to_owned());
        self.run(cx, future, move |this, result, cx| {
            this.friends.busy.remove(&id);
            match result {
                Ok(state) => {
                    let friends = "You'll see each other online.";
                    let done = match act {
                        Act::Request if state == FRIEND => Some((format!("You and {name} are friends"), friends)),
                        Act::Request => {
                            Some((format!("Friend request sent to {name}"), "It waits for them in their friends list."))
                        }
                        Act::Accept => Some((format!("You and {name} are friends"), friends)),
                        Act::Remove => None,
                        Act::Block => Some((format!("Blocked {name}"), "They won't be told.")),
                        Act::Unblock => {
                            Some((format!("Unblocked {name}"), "They can reach you again, as your settings allow."))
                        }
                    };
                    if let Some((title, body)) = done {
                        let glyph = if act == Act::Block { "ban" } else { "user-check" };
                        this.toast(glyph, title, body.into(), None, None, cx);
                    }
                    this.load_relation(&k, &id, cx);
                }
                Err(err) => this.toast("circle-alert", "That didn't work".into(), err.message, None, None, cx),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Asks the instance where you stand with someone, for their profile card.
    pub(crate) fn load_relation(&mut self, key: &str, user_id: &str, cx: &mut Context<Self>) {
        let (core, k, id) = (self.core.clone(), key.to_owned(), user_id.to_owned());
        self.run(cx, async move { core.relationship(&k, &id).await }, {
            let id = user_id.to_owned();
            move |this, result, cx| {
                if let Ok(relation) = result {
                    this.friends.relation = Some((id, relation));
                    cx.notify();
                }
            }
        });
    }

    fn send_typed_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = friends::clean_username(&self.friends.add.read(cx).value());
        let Some(key) = (match &self.nav {
            crate::ui::app::Nav::Friends { key } => Some(key.clone()),
            _ => None,
        }) else {
            return;
        };
        if typed.is_empty() || self.friends.sending {
            return;
        }
        self.friends.sending = true;
        self.friends.result = None;
        let (core, name) = (self.core.clone(), typed.clone());
        self.run(cx, async move { core.send_friend_request(&key, "", &name).await }, move |this, result, cx| {
            this.friends.sending = false;
            this.friends.result = Some(match result {
                Ok(f) if f.state == FRIEND => (true, format!("You and @{typed} are friends now")),
                Ok(_) => (true, format!("Request sent to @{typed}")),
                Err(err) => (false, err.message),
            });
            if this.friends.result.as_ref().is_some_and(|(ok, _)| *ok) {
                this.friends.clear_add = true;
            }
            cx.notify();
        });
        window.refresh();
        cx.notify();
    }

    pub(crate) fn friends_view(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let now = now_ms();
        let Some((status, list, instance, several)) = self.core.shared.read(|s| {
            let several = s.order.iter().filter_map(|k| s.instance(k)).filter(|i| i.me.is_some()).count() > 1;
            s.instance(key).map(|i| (i.friends.status, i.friends.list.clone(), i.name(), several))
        }) else {
            return div().into_any_element();
        };
        if self.friends.opened.as_deref() != Some(key) {
            self.friends.opened = Some(key.to_owned());
            self.friends.tab = if friends::waiting_for_you(&list, now) > 0 { Tab::Pending } else { Tab::Online };
            self.friends.adding = false;
            self.friends.result = None;
            self.friends.search.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if std::mem::take(&mut self.friends.clear_add) {
            self.friends.add.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let counts = |tab| match tab {
            Tab::Pending => friends::waiting_for_you(&list, now),
            tab => friends::in_tab(&list, tab, "", now).len(),
        };

        // The header: what this is, the tabs on a gliding pill, and Add friend.
        const TAB_W: f32 = 92.0;
        let current = Tab::ALL.iter().position(|t| *t == self.friends.tab).unwrap_or(0);
        let pill = motion::follow("friends-tab-pill", current as f32 * TAB_W, window, cx);
        let mut tabs = div().relative().flex().h(px(32.0)).child(
            div()
                .absolute()
                .top_0()
                .left(px(pill))
                .w(px(TAB_W - 4.0))
                .h(px(32.0))
                .rounded(corner(10.0))
                .bg(alpha(p.primary, 0.14)),
        );
        for (n, tab) in Tab::ALL.into_iter().enumerate() {
            let on = n == current;
            let count = counts(tab);
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("friends-tab-{}", tab.label())))
                    .relative()
                    .w(px(TAB_W - 4.0))
                    .mr(px(4.0))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .rounded(corner(10.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .text_color(if on { p.primary } else { p.muted_foreground })
                    .when(!on, |el| el.hover(|s| s.bg(p.muted)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.friends.tab = tab;
                        cx.notify();
                    }))
                    .child(tab.label())
                    .when(tab == Tab::Pending && count > 0, |el| {
                        el.child(badge(count as u32, &p).border_color(p.chat_surface))
                    }),
            );
        }
        let adding = self.friends.adding;
        let k = key.to_owned();
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(20.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("users").size(px(20.0)).text_color(p.primary))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Friends"))
            .when(several && !self.prefs.streamer_mode, |el| {
                el.child(div().text_sm().text_color(p.muted_foreground).child(instance))
            })
            .child(div().w(px(1.0)).h(px(24.0)).bg(p.border))
            .child(tabs)
            .child(div().flex_1())
            .child(
                if adding {
                    soft_button("friends-add", "Close", &p).child(icon("x").size(px(16.0)))
                } else {
                    primary_button("friends-add", "Add friend", &p)
                        .h(px(36.0))
                        .text_sm()
                        .child(icon("user-plus").size(px(16.0)))
                }
                .on_click(cx.listener(|this, _, window, cx| {
                    this.friends.adding = !this.friends.adding;
                    this.friends.result = None;
                    if this.friends.adding {
                        this.friends.add.update(cx, |s, cx| s.focus(window, cx));
                    }
                    cx.notify();
                })),
            )
            .child(
                icon_button("friends-settings", "settings", &p)
                    .on_click(cx.listener(move |this, _, window, cx| this.open_friend_settings(&k, window, cx))),
            );

        let mut page = div().flex_1().min_h_0().flex().flex_col().child(header);
        if status == FriendsStatus::Unsupported {
            return page
                .child(crate::ui::search::empty(
                    "sparkles",
                    "Friends aren't here yet",
                    "This instance runs a version of fuwa from before friends. They'll show up once it updates.",
                    &p,
                ))
                .into_any_element();
        }
        if adding {
            page = page.child(self.add_friend_box(&p, cx));
        }
        let typed = self.friends.search.read(cx).value().to_string();
        let shown = friends::in_tab(&list, self.friends.tab, &typed, now);
        page = page.child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(12.0))
                .px(px(20.0))
                .pt(px(14.0))
                .pb(px(4.0))
                .child(
                    div().w(px(280.0)).child(
                        Input::new(&self.friends.search)
                            .prefix(icon("search").size(px(14.0)).text_color(p.muted_foreground)),
                    ),
                )
                .child(div().flex_1())
                .child(
                    div().text_xs().font_weight(FontWeight::EXTRA_BOLD).text_color(p.muted_foreground).child(format!(
                        "{} — {}",
                        self.friends.tab.label().to_uppercase(),
                        shown.len()
                    )),
                ),
        );

        let mut body = div().id("friends-list").flex_1().min_h_0().overflow_y_scroll().px(px(12.0)).pb(px(16.0));
        if status != FriendsStatus::Ready && list.is_empty() {
            body = body.child(crate::ui::search::skeleton(4, &p));
        } else if shown.is_empty() {
            let (glyph, title, text) = match (typed.trim().is_empty(), self.friends.tab) {
                (false, _) => ("search", "Nobody by that name", "Try part of their name or username."),
                (true, Tab::Blocked) => (
                    "ban",
                    "Nobody blocked",
                    "People you block can't message you or send you requests, and they're never told.",
                ),
                (true, Tab::Pending) => (
                    "heart-handshake",
                    "No requests waiting",
                    "Requests you send and get show up here until they're answered.",
                ),
                (true, Tab::Online) => {
                    ("users", "Nobody's around right now", "Friends show up here while they have fuwa open.")
                }
                (true, Tab::All) => (
                    "users",
                    "No friends here yet",
                    "Add someone by their username, or from their profile in a server you share.",
                ),
            };
            body = body.child(crate::ui::search::empty(glyph, title, text, &p));
        } else {
            for (n, f) in shown.into_iter().enumerate() {
                body = body.child(self.friend_line(key, f, n, now, &p, cx));
            }
        }
        page.child(body).into_any_element()
    }

    fn add_friend_box(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let typed = !friends::clean_username(&self.friends.add.read(cx).value()).is_empty();
        let sending = self.friends.sending;
        let result = self.friends.result.clone();
        motion::slide_in(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(20.0))
                .py(px(16.0))
                .border_b_1()
                .border_color(p.border)
                .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Add a friend"))
                .child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child("Ask someone on this instance by their username. Only the two of you will know."),
                )
                .child({
                    let row = div()
                        .mt(px(8.0))
                        .max_w(px(560.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().flex_1().child(
                            Input::new(&self.friends.add).prefix(div().text_color(p.muted_foreground).child("@")),
                        ))
                        .child(
                            primary_button("friends-send", if sending { "Sending…" } else { "Send request" }, p)
                                .h(px(36.0))
                                .text_sm()
                                .when(!typed || sending, |el| el.opacity(0.5))
                                .child(icon(if sending { "loader-circle" } else { "user-plus" }).size(px(16.0)))
                                .on_click(cx.listener(|this, _, window, cx| this.send_typed_request(window, cx))),
                        );
                    // A request that didn't go gives the box a little shake.
                    match result.as_ref().filter(|(ok, _)| !ok) {
                        Some((_, text)) => motion::once(
                            row,
                            SharedString::from(format!("friend-shake-{text}")),
                            Duration::from_millis(400),
                            |el, t| el.ml(px((t * std::f32::consts::TAU * 3.0).sin() * 8.0 * (1.0 - t))),
                        ),
                        None => row.into_any_element(),
                    }
                })
                .when_some(result, |el, (ok, text)| {
                    el.child(motion::rise(
                        div()
                            .mt(px(6.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if ok { p.success } else { p.destructive })
                            .when(ok, |el| el.child(icon("check").size(px(16.0))))
                            .child(text.clone()),
                        SharedString::from(format!("friend-result-{text}")),
                        Duration::ZERO,
                        4.0,
                    ))
                }),
            "friends-add-box",
            -8.0,
        )
        .into_any_element()
    }

    /// One person in a tab: who they are, how things stand, and what you can do.
    fn friend_line(
        &mut self,
        key: &str,
        f: &pb::Friend,
        n: usize,
        now: i64,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(user) = f.user.clone() else { return div().into_any_element() };
        let id = user.id.clone();
        let name = user_name(&user);
        let busy = self.friends.busy.contains(&id);
        let line = if f.state == INCOMING || f.state == OUTGOING {
            friends::pending_line(f, now)
        } else if f.state == BLOCKED {
            "Blocked: they can't message you or send requests".to_owned()
        } else if !user.status.is_empty() {
            user.status.clone()
        } else if f.online {
            "Online".to_owned()
        } else {
            "Offline".to_owned()
        };
        let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == format!("member|{id}"));
        let hover = alpha(p.primary, 0.06);
        let row_id = SharedString::from(format!("friend|{id}"));
        let (k, uid) = (key.to_owned(), id.clone());
        let act = |name: &'static str, glyph: &'static str, color, act: Act, cx: &mut Context<Self>| {
            let (k, uid) = (key.to_owned(), id.clone());
            icon_button_in(SharedString::from(format!("{name}|{id}")), glyph, p, color)
                .size(px(36.0))
                .rounded_full()
                .bg(p.secondary)
                .when(busy, |el| el.opacity(0.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.friend_act(&k, &uid, act, cx);
                }))
        };
        let mut actions = div().flex().items_center().gap(px(8.0));
        if f.state == FRIEND {
            let (k, uid) = (key.to_owned(), id.clone());
            actions = actions
                .child(
                    icon_button(SharedString::from(format!("friend-message|{id}")), "message-circle", p)
                        .size(px(36.0))
                        .rounded_full()
                        .bg(p.secondary)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.message_person(k.clone(), uid.clone(), window, cx);
                        })),
                )
                .child(act("friend-remove", "user-minus", p.destructive, Act::Remove, cx))
                .child(act("friend-block", "ban", p.destructive, Act::Block, cx));
        } else if f.state == INCOMING {
            actions = actions
                .child(
                    act("friend-accept", "check", p.success, Act::Accept, cx)
                        .bg(alpha(p.success, 0.16))
                        .text_color(p.success),
                )
                .child(act("friend-decline", "x", p.destructive, Act::Remove, cx));
        } else if f.state == OUTGOING {
            actions = actions.child(act("friend-cancel", "x", p.destructive, Act::Remove, cx));
        } else if f.state == BLOCKED {
            let (k, uid) = (key.to_owned(), id.clone());
            actions = actions.child(
                soft_button(SharedString::from(format!("friend-unblock|{id}")), "Unblock", p)
                    .child(icon("shield-off").size(px(14.0)))
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.friend_act(&k, &uid, Act::Unblock, cx);
                    })),
            );
        }
        let dot = (f.state == FRIEND).then(|| {
            div()
                .absolute()
                .right(px(-2.0))
                .bottom(px(-2.0))
                .size(px(14.0))
                .rounded_full()
                .border_3()
                .border_color(p.chat_surface)
                .bg(if f.online { p.success.into() } else { alpha(p.muted_foreground, 0.5) })
        });
        motion::rise(
            div()
                .id(row_id.clone())
                .h(px(62.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .border_t_1()
                .border_color(alpha(p.border, 0.6))
                .rounded(corner(12.0))
                .cursor_pointer()
                .when(lit, |el| el.bg(hover))
                .hover(move |s| s.bg(hover))
                .on_mouse_down(
                    MouseButton::Right,
                    self.right_click(MenuOf::Member { key: k.clone(), server: None, user_id: uid.clone() }, cx),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(Dialog::Profile { key: k.clone(), user_id: uid.clone(), server: None }, window, cx)
                }))
                .child(div().relative().child(avatar(Some(&user), 36.0, p)).children(dot))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(6.0))
                                .child(div().font_weight(FontWeight::BOLD).child(name))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(p.muted_foreground)
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(format!("@{}", user.username)),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.muted_foreground)
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(line),
                        ),
                )
                .child(actions),
            SharedString::from(format!("friend-in|{id}")),
            Duration::from_millis(18 * n.min(12) as u64),
            6.0,
        )
        .into_any_element()
    }

    /// The friend buttons on someone's profile card: add them, take or turn
    /// down their request, cancel yours, unfriend, block; and the friends you
    /// share, when everyone involved allows it. None for you, agents, or an
    /// instance without friends.
    pub(crate) fn profile_friend_buttons(
        &mut self,
        key: &str,
        user_id: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = now_ms();
        let (status, state) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            Some((i.friends.status, friends::state_with(&i.friends.list, user_id, now)))
        })?;
        if !matches!(status, FriendsStatus::Ready | FriendsStatus::Loading) {
            return None;
        }
        let relation = self.friends.relation.as_ref().filter(|(id, _)| id == user_id).map(|(_, r)| r.clone());
        let mutual = relation.as_ref().map(|r| r.mutual_friends.clone()).unwrap_or_default();
        let may_request = relation.as_ref().is_none_or(|r| r.may_request);
        let busy = self.friends.busy.contains(user_id);
        let button = |id: &'static str, glyph: &'static str, label: String, tone: Option<gpui_kit::Rgba>| {
            let mut b = div()
                .id(id)
                .flex_1()
                .min_w_0()
                .h(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .rounded(corner(12.0))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .when(busy, |el| el.opacity(0.6))
                .child(icon(glyph).size(px(16.0)))
                .child(div().whitespace_nowrap().text_ellipsis().child(label));
            b = match tone {
                Some(c) => b.bg(alpha(c, 0.16)).text_color(c).hover(move |s| s.bg(alpha(c, 0.26))),
                None => b.bg(p.secondary).hover({
                    let c = mix(p.secondary, p.primary, 0.14);
                    move |s| s.bg(c)
                }),
            };
            b
        };
        let on = |act: Act, cx: &mut Context<Self>| act_on(key, user_id, act, cx);
        let mut row = div().flex().gap(px(6.0));
        let main = match state {
            0 if may_request => {
                button("friend-add", if busy { "loader-circle" } else { "user-plus" }, "Add friend".into(), None)
                    .on_click(on(Act::Request, cx))
                    .into_any_element()
            }
            0 => button("friend-add", "user-plus", "Not taking requests".into(), None)
                .opacity(0.5)
                .cursor_default()
                .into_any_element(),
            OUTGOING => swap(
                button("friend-requested", "clock", "Requested".into(), None),
                "friend-requested",
                "x",
                "Cancel request",
                p,
            )
            .on_click(on(Act::Remove, cx))
            .into_any_element(),
            INCOMING => div()
                .flex_1()
                .flex()
                .gap(px(6.0))
                .child(button("friend-accept", "check", "Accept".into(), Some(p.success)).on_click(on(Act::Accept, cx)))
                .child(button("friend-decline", "x", "Decline".into(), None).on_click(on(Act::Remove, cx)))
                .into_any_element(),
            FRIEND => swap(
                button("friend-friends", "user-check", "Friends".into(), None),
                "friend-friends",
                "user-minus",
                "Remove friend",
                p,
            )
            .on_click(on(Act::Remove, cx))
            .into_any_element(),
            _ => button("friend-unblock", "shield-off", "Unblock".into(), None)
                .on_click(on(Act::Unblock, cx))
                .into_any_element(),
        };
        row = row.child(main);
        if state != BLOCKED {
            row = row.child(
                icon_button_in("friend-block", "ban", p, p.destructive)
                    .size(px(36.0))
                    .rounded(corner(12.0))
                    .on_click(on(Act::Block, cx)),
            );
        }
        let mut out = div().flex().flex_col().gap(px(6.0));
        if !mutual.is_empty() {
            let text = if mutual.len() == 1 {
                format!("{} is a friend of you both", user_name(&mutual[0]))
            } else {
                format!("{} mutual friends", mutual.len())
            };
            out = out.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(2.0))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(div().flex().children(mutual.iter().take(3).enumerate().map(|(n, u)| {
                        div()
                            .when(n > 0, |el| el.ml(px(-6.0)))
                            .rounded_full()
                            .border_2()
                            .border_color(p.card)
                            .child(avatar(Some(u), 20.0, p))
                    })))
                    .child(div().whitespace_nowrap().text_ellipsis().child(text)),
                SharedString::from(format!("mutual-{user_id}")),
                Duration::ZERO,
                4.0,
            ));
        }
        Some(out.child(row).into_any_element())
    }
}

/// A click that does something about someone.
fn act_on(
    key: &str,
    user_id: &str,
    act: Act,
    cx: &mut Context<FuwaApp>,
) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static + use<> {
    let (k, id, app) = (key.to_owned(), user_id.to_owned(), cx.entity().downgrade());
    move |_, _, cx| {
        let _ = app.update(cx, |this, cx| this.friend_act(&k, &id, act, cx));
    }
}

/// A button whose label turns into what clicking it does while hovered
/// ("Friends" becomes "Remove friend"), in red.
fn swap(
    button: gpui_kit::Stateful<gpui_kit::Div>,
    group: &'static str,
    glyph: &'static str,
    label: &'static str,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let red = p.destructive;
    button.group(group).relative().overflow_hidden().child(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .opacity(0.0)
            .bg(mix(p.card, red, 0.12))
            .text_color(red)
            .group_hover(group, move |s| s.opacity(1.0))
            .child(icon(glyph).size(px(16.0)))
            .child(label),
    )
}
