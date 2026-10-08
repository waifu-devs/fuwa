//! Friends under Home (docs/friends.md): tabs for who's online, everyone,
//! requests and blocks, a box to ask someone by username, and the buttons on
//! a profile card. Like the web app's `pages/FriendsPage.tsx` and
//! `components/friends/FriendActions.tsx`.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, point, px, rgb,
};

use crate::core::dms::now_ms;
use crate::core::friends::{self, BLOCKED, FRIEND, FriendsStatus, INCOMING, OUTGOING, Tab};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::store::user_name;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::context_menu::MenuOf;
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_md, radius_sm, radius_xl};
use crate::ui::widgets::{avatar, icon, pal};

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
    /// A friend's "more" menu, open under its button.
    pub more: Option<(String, gpui_kit::Point<gpui_kit::Pixels>)>,
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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("dms-calls.friends.page.search")));
        let add = cx.new(|cx| InputState::new(window, cx).placeholder(t("dms-calls.friends.page.username")));
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
                more: None,
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
            .unwrap_or_default();
        let was = self
            .core
            .shared
            .read(|s| s.instance(key).map(|i| friends::state_with(&i.friends.list, user_id, now_ms())).unwrap_or(0));
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
                    let name = Arg::Str(&name);
                    let done = match act {
                        Act::Request if state == FRIEND => {
                            Some(t_with("dms-calls.friends.actions.nowFriends", &[("name", name)]))
                        }
                        Act::Request => Some(t_with("dms-calls.friends.actions.sent", &[("name", name)])),
                        Act::Accept => Some(t_with("dms-calls.friends.actions.nowFriends", &[("name", name)])),
                        Act::Remove if was == FRIEND => Some(t_with("dms-calls.friends.removed", &[("name", name)])),
                        Act::Remove => None,
                        Act::Block => Some(t_with("dms-calls.friends.actions.blocked", &[("name", name)])),
                        Act::Unblock => Some(t_with("dms-calls.friends.unblocked", &[("name", name)])),
                    };
                    if let Some(title) = done {
                        let glyph = if act == Act::Block { "ban" } else { "user-check" };
                        this.toast(glyph, title, String::new(), None, None, cx);
                    }
                    this.load_relation(&k, &id, cx);
                }
                Err(err) => this.toast("circle-alert", err.message, String::new(), None, None, cx),
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
                Ok(f) if f.state == FRIEND => {
                    (true, t_with("dms-calls.friends.page.nowFriends", &[("username", Arg::Str(&typed))]))
                }
                Ok(_) => (true, t_with("dms-calls.friends.page.requestSent", &[("username", Arg::Str(&typed))])),
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
        let Some((status, list, signed_in, dms)) = self.core.shared.read(|s| {
            s.instance(key).map(|i| {
                (
                    i.friends.status,
                    i.friends.list.clone(),
                    i.me.is_some(),
                    matches!(i.dms.status, crate::core::dms::DmStatus::Ready | crate::core::dms::DmStatus::Starting),
                )
            })
        }) else {
            return div().into_any_element();
        };
        if self.friends.opened.as_deref() != Some(key) {
            self.friends.opened = Some(key.to_owned());
            self.friends.tab = if friends::waiting_for_you(&list, now) > 0 { Tab::Pending } else { Tab::Online };
            self.friends.adding = false;
            self.friends.result = None;
            self.friends.more = None;
            self.friends.search.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if std::mem::take(&mut self.friends.clear_add) {
            self.friends.add.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let counts = |tab| match tab {
            Tab::Pending => friends::waiting_for_you(&list, now),
            tab => friends::in_tab(&list, tab, "", now).len(),
        };

        // The header (`h-14 border-b px-4`): what this is, the tabs on a gliding pill, and Add friend.
        let adding = self.friends.adding;
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(icon("users").size(px(20.0)).text_color(p.primary))
                    .child(t("dms-calls.friends.title")),
            )
            .child(div().mx(px(4.0)).w(px(1.0)).h(px(24.0)).flex_none().bg(p.border))
            .child(self.friend_tabs(&counts, &p, window, cx))
            .child({
                let (bg, fg) = if adding { (p.muted, p.foreground) } else { (p.primary, p.primary_foreground) };
                let label = if adding { t("common.close") } else { t("dms-calls.friends.add") };
                div()
                    .id("friends-add")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .px(px(12.0))
                    .py(px(6.0))
                    .bg(bg)
                    .text_color(fg)
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.92))
                    .active(|s| s.top(px(1.0)))
                    // The web turns its X a quarter of the way round: a plus.
                    .child(icon(if adding { "plus" } else { "user-plus" }).size(px(16.0)))
                    .child(label)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.friends.adding = !this.friends.adding;
                        this.friends.result = None;
                        if this.friends.adding {
                            this.friends.add.update(cx, |s, cx| s.focus(window, cx));
                        }
                        cx.notify();
                    }))
            });

        let mut page = div().relative().flex_1().min_h_0().flex().flex_col().child(header);
        if adding {
            page = page.child(self.add_friend_box(&p, window, cx));
        }
        if !signed_in {
            return page.into_any_element();
        }
        if status == FriendsStatus::Unsupported {
            return page
                .child(empty(
                    "sparkles",
                    &t("dms-calls.friends.page.unsupportedTitle"),
                    &t("dms-calls.friends.page.unsupportedText"),
                    None,
                    &p,
                    window,
                ))
                .into_any_element();
        }
        let typed = self.friends.search.read(cx).value().to_string();
        let searching = self.friends.search.read(cx).focus_handle(cx).is_focused(window);
        let shown = friends::in_tab(&list, self.friends.tab, &typed, now);
        let tab_label = t(tab_key(self.friends.tab));
        let fg = p.foreground;
        let muted = p.muted;
        page = page.child(
            div()
                .flex_none()
                .px(px(24.0))
                .pt(px(12.0))
                .child(
                    div()
                        .h(px(38.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_xl())
                        .border_1()
                        .map(|el| focus_ring(el, searching, &p))
                        .bg(p.card)
                        .px(px(12.0))
                        .text_sm()
                        .child(icon("search").size(px(16.0)).text_color(p.muted_foreground))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.friends.search).appearance(false)))
                        .when(!typed.is_empty(), |el| {
                            el.child(
                                div()
                                    .id("friends-search-clear")
                                    .size(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .text_color(p.muted_foreground)
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(muted).text_color(fg))
                                    .child(icon("x").size(px(14.0)))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.friends.search.update(cx, |s, cx| s.set_value("", window, cx));
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .mb(px(4.0))
                        .px(px(4.0))
                        .text_xs()
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(tracked(
                            t_with(
                                "dms-calls.friends.page.heading",
                                &[("tab", Arg::Str(&tab_label)), ("count", Arg::Num(shown.len() as i64))],
                            )
                            .to_uppercase(),
                            WIDE,
                        )),
                ),
        );

        let tab_id = tab_key(self.friends.tab);
        let mut body = div().id("friends-list").flex_1().min_h_0().overflow_y_scroll().px(px(24.0)).pb(px(24.0));
        if status != FriendsStatus::Ready && list.is_empty() {
            body = body.child(loading(&p));
        } else if shown.is_empty() {
            let searching = !typed.trim().is_empty();
            let (glyph, title, text) = match (searching, self.friends.tab) {
                (true, _) => ("search", "dms-calls.friends.page.noMatchTitle", "dms-calls.friends.page.noMatchText"),
                (false, Tab::Blocked) => {
                    ("ban", "dms-calls.friends.page.noBlockedTitle", "dms-calls.friends.page.noBlockedText")
                }
                (false, Tab::Pending) => {
                    ("heart-handshake", "dms-calls.friends.page.noPendingTitle", "dms-calls.friends.page.noPendingText")
                }
                (false, Tab::Online) => {
                    ("users", "dms-calls.friends.page.noOnlineTitle", "dms-calls.friends.page.noOnlineText")
                }
                (false, Tab::All) => {
                    ("users", "dms-calls.friends.page.noFriendsTitle", "dms-calls.friends.page.noFriendsText")
                }
            };
            let action = (!searching && self.friends.tab == Tab::All).then(|| {
                let (bg, fg) = (p.primary, p.primary_foreground);
                div()
                    .id("friends-empty-add")
                    .mt(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .bg(bg)
                    .text_color(fg)
                    .px(px(16.0))
                    .py(px(8.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.92))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("user-plus").size(px(16.0)))
                    .child(t("dms-calls.friends.add"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.friends.adding = true;
                        this.friends.add.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    }))
                    .into_any_element()
            });
            body = body.child(empty(glyph, &t(title), &t(text), action, &p, window));
        } else {
            for (n, f) in shown.iter().enumerate() {
                body = body.child(self.friend_line(key, f, n, dms, &p, cx));
            }
        }
        // A new tab slides in from the side.
        page = page.child(motion::slide_in(
            div().flex_1().min_h_0().flex().flex_col().child(body),
            SharedString::from(format!("friends-tab-in|{tab_id}")),
            12.0,
        ));
        if let Some((user, at)) = self.friends.more.clone() {
            page = page.child(self.friend_more_menu(key, &user, at, &p, cx));
        }
        page.into_any_element()
    }

    /// The four tabs, the picked one on a pill that glides between them, with
    /// how many are online and in all, and the requests waiting.
    fn friend_tabs(
        &mut self,
        counts: &dyn Fn(Tab) -> usize,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.friends.tab;
        let placed = TAB_PLACES.with(|places| places.borrow().clone());
        let origin = placed.get(&None).map(|b| b.0);
        let pill = origin.and_then(|o| placed.get(&Some(current)).map(|b| (b.0 - o, b.1)));
        let mut nav = div().relative().flex().flex_1().min_w_0().items_center().gap(px(4.0)).child(
            gpui_kit::canvas(
                |bounds, _, _| {
                    TAB_PLACES.with(|places| {
                        places.borrow_mut().insert(None, (f32::from(bounds.left()), f32::from(bounds.size.width)))
                    })
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        if let Some((x, w)) = pill {
            let x = motion::follow("friends-pill-x", x, window, cx);
            let w = motion::follow("friends-pill-w", w, window, cx);
            nav = nav.child(
                div().absolute().left(px(x)).top_0().w(px(w)).h(px(32.0)).rounded_full().bg(alpha(p.primary, 0.15)),
            );
        }
        for tab in Tab::ALL {
            let on = tab == current;
            let count = counts(tab);
            let fg = p.foreground;
            nav = nav.child(
                div()
                    .id(SharedString::from(format!("friends-tab-{}", tab.label())))
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(32.0))
                    .px(px(12.0))
                    .rounded_full()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .when(on && pill.is_none(), |el| el.bg(alpha(p.primary, 0.15)))
                    .text_color(if on { p.primary } else { p.muted_foreground })
                    .when(!on, |el| el.hover(move |s| s.text_color(fg)))
                    .active(|s| s.top(px(1.0)))
                    .child(
                        gpui_kit::canvas(
                            move |bounds, _, _| {
                                TAB_PLACES.with(|places| {
                                    places
                                        .borrow_mut()
                                        .insert(Some(tab), (f32::from(bounds.left()), f32::from(bounds.size.width)))
                                })
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(t(tab_key(tab)))
                    .when(tab == Tab::Pending && count > 0, |el| {
                        el.child(motion::rise(
                            div()
                                .h(px(20.0))
                                .min_w(px(20.0))
                                .px(px(6.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(p.primary)
                                .text_color(p.primary_foreground)
                                .text_size(px(11.2))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(if count > 99 { "99+".to_owned() } else { count.to_string() }),
                            SharedString::from(format!("friends-badge|{count}")),
                            Duration::ZERO,
                            2.0,
                        ))
                    })
                    .when(matches!(tab, Tab::Online | Tab::All) && count > 0, |el| {
                        el.child(div().text_xs().opacity(0.7).child(count.to_string()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.friends.tab = tab;
                        this.friends.more = None;
                        cx.notify();
                    })),
            );
        }
        if pill.is_none() {
            // The pill glides once the tabs have been measured, on the next frame.
            window.request_animation_frame();
        }
        nav.into_any_element()
    }

    fn add_friend_box(&mut self, p: &Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.friends.add.read(cx).focus_handle(cx).is_focused(window);
        let typed = !friends::clean_username(&self.friends.add.read(cx).value()).is_empty();
        let sending = self.friends.sending;
        let result = self.friends.result.clone();
        let emerald: gpui_kit::Hsla = if p.dark { rgb(0x34d399).into() } else { rgb(0x059669).into() };
        let field = div()
            .mt(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_2xl())
            .border_1()
            .map(|el| focus_ring(el, focused, p))
            .bg(p.card)
            .p(px(6.0))
            .pl(px(12.0))
            .child(div().text_color(p.muted_foreground).child("@"))
            .child(div().flex_1().min_w_0().text_sm().child(Input::new(&self.friends.add).appearance(false)))
            .child(
                div()
                    .id("friends-send")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(radius_xl())
                    .px(px(12.0))
                    .py(px(8.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    // `disabled:opacity-50` fades the button as one piece over the card: the
                    // fill shows half, the text over a half-faded fill (GPUI fades each alone).
                    .map(|el| {
                        if !typed || sending {
                            el.bg(alpha(p.primary, 0.5)).text_color(crate::ui::theme::mix(
                                p.card,
                                p.primary_foreground,
                                0.5,
                            ))
                        } else {
                            el.bg(p.primary).text_color(p.primary_foreground)
                        }
                    })
                    .when(typed && !sending, |el| {
                        el.cursor_pointer().hover(|s| s.opacity(0.92)).active(|s| s.top(px(1.0)))
                    })
                    .child(icon(if sending { "loader-circle" } else { "user-plus" }).size(px(16.0)))
                    .child(t("dms-calls.friends.page.sendRequest"))
                    .on_click(cx.listener(|this, _, window, cx| this.send_typed_request(window, cx))),
            );
        // A request that didn't go gives the box a little shake.
        let field = match result.as_ref().filter(|(ok, _)| !ok) {
            Some((_, text)) => motion::once(
                field,
                SharedString::from(format!("friend-shake-{text}")),
                Duration::from_millis(400),
                |el, t| el.ml(px((t * std::f32::consts::TAU * 3.0).sin() * 8.0 * (1.0 - t))),
            ),
            None => field.into_any_element(),
        };
        motion::rise(
            div()
                .flex_none()
                .px(px(24.0))
                .py(px(16.0))
                .border_b_1()
                .border_color(p.border)
                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("dms-calls.friends.page.addTitle")))
                .child(
                    div()
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(t("dms-calls.friends.page.addText")),
                )
                .child(field)
                .when_some(result, |el, (ok, text)| {
                    el.child(motion::rise(
                        div()
                            .mt(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if ok { emerald } else { p.destructive.into() })
                            .when(ok, |el| el.child(icon("check").size(px(16.0))))
                            .child(text.clone()),
                        SharedString::from(format!("friend-result-{text}")),
                        Duration::ZERO,
                        -4.0,
                    ))
                }),
            "friends-add-box",
            Duration::ZERO,
            -12.0,
        )
        .into_any_element()
    }

    /// One person in a tab: who they are, how things stand, and what you can do.
    fn friend_line(
        &mut self,
        key: &str,
        f: &pb::Friend,
        n: usize,
        dms: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(user) = f.user.clone() else { return div().into_any_element() };
        let now = now_ms();
        let id = user.id.clone();
        let name = user_name(&user);
        let busy = self.friends.busy.contains(&id);
        let line = if f.state == INCOMING || f.state == OUTGOING {
            friends::pending_line(f, now)
        } else if f.state == BLOCKED {
            t("dms-calls.friends.page.blockedLine")
        } else if let Some(status) = crate::ui::presence::custom_status(&user, now) {
            status
        } else if f.online {
            t("dms-calls.friends.page.online")
        } else {
            t("dms-calls.friends.page.offline")
        };
        let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == format!("member|{id}"));
        let hover = alpha(p.muted, 0.6);
        let (k, uid) = (key.to_owned(), id.clone());
        let emerald_bg = rgb(0x10b981);
        let emerald_fg: gpui_kit::Hsla = if p.dark { rgb(0x34d399).into() } else { rgb(0x059669).into() };
        let round = |name: &str, glyph: &'static str, tone: Option<bool>, cx: &mut Context<Self>, act: Act| {
            let (k, uid) = (key.to_owned(), id.clone());
            let (bg, fg) = match tone {
                Some(true) => (alpha(emerald_bg, 0.15), emerald_fg),
                Some(false) => (alpha(p.destructive, 0.1), p.destructive.into()),
                None => (p.muted.into(), p.foreground.into()),
            };
            div()
                .id(SharedString::from(format!("{name}|{id}")))
                .size(px(36.0))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(alpha(p.muted, 0.7))
                .text_color(p.muted_foreground)
                .cursor_pointer()
                .when(busy, |el| el.opacity(0.6))
                .hover(move |s| s.bg(bg).text_color(fg))
                .active(|s| s.top(px(1.0)))
                .child(icon(glyph).size(px(16.0)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.friend_act(&k, &uid, act, cx);
                }))
        };
        let mut actions = div().flex().flex_none().items_center().gap(px(6.0));
        if f.state == FRIEND {
            if dms {
                let (k, uid) = (key.to_owned(), id.clone());
                let (bg, fg) = (p.muted, p.foreground);
                actions = actions.child(
                    div()
                        .id(SharedString::from(format!("friend-message|{id}")))
                        .size(px(36.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(alpha(p.muted, 0.7))
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(bg).text_color(fg))
                        .active(|s| s.top(px(1.0)))
                        .child(icon("message-circle").size(px(16.0)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.message_person(k.clone(), uid.clone(), window, cx);
                        })),
                );
            }
            let open = self.friends.more.as_ref().is_some_and(|(u, _)| *u == id);
            let (bg, fg) = (p.muted, p.foreground);
            let uid = id.clone();
            actions = actions.child(
                div()
                    .id(SharedString::from(format!("friend-more|{id}")))
                    .relative()
                    .size(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(if open { p.muted.into() } else { alpha(p.muted, 0.7) })
                    .text_color(if open { p.foreground } else { p.muted_foreground })
                    .cursor_pointer()
                    .hover(move |s| s.bg(bg).text_color(fg))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("ellipsis-vertical").size(px(16.0)))
                    .child({
                        let uid = id.clone();
                        gpui_kit::canvas(
                            move |bounds, _, _| MORE_PLACES.with(|m| m.borrow_mut().insert(uid, bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        let at = MORE_PLACES
                            .with(|m| m.borrow().get(&uid).copied())
                            .map(|b| point(b.right(), b.bottom() + px(4.0)))
                            .unwrap_or_else(|| window.mouse_position());
                        this.friends.more = match &this.friends.more {
                            Some((u, _)) if *u == uid => None,
                            _ => Some((uid.clone(), at)),
                        };
                        cx.notify();
                    })),
            );
        } else if f.state == INCOMING {
            actions = actions.child(round("friend-accept", "check", Some(true), cx, Act::Accept)).child(round(
                "friend-decline",
                "x",
                Some(false),
                cx,
                Act::Remove,
            ));
        } else if f.state == OUTGOING {
            actions = actions.child(round("friend-cancel", "x", Some(false), cx, Act::Remove));
        } else if f.state == BLOCKED {
            let (k, uid) = (key.to_owned(), id.clone());
            let (bg, fg) = (p.muted, p.foreground);
            actions = actions.child(
                div()
                    .id(SharedString::from(format!("friend-unblock|{id}")))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .bg(alpha(p.muted, 0.7))
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .when(busy, |el| el.opacity(0.6))
                    .hover(move |s| s.bg(bg).text_color(fg))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("shield-off").size(px(14.0)))
                    .child(t("dms-calls.friends.unblock"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.friend_act(&k, &uid, Act::Unblock, cx);
                    })),
            );
        }
        let dot = (f.state == FRIEND).then(|| {
            div()
                .absolute()
                .right(px(-2.0 - 3.0))
                .bottom(px(-2.0 - 3.0))
                .size(px(14.0 + 6.0))
                .rounded_full()
                .border(px(3.0))
                .border_color(p.background)
                .child(div().size_full().rounded_full().bg(if f.online {
                    emerald_bg.into()
                } else {
                    alpha(p.muted_foreground, 0.5)
                }))
        });
        let group = SharedString::from(format!("friend|{id}"));
        let who = div()
            .id(group.clone())
            .group(group.clone())
            .relative()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(radius_xl())
            .px(px(4.0))
            .py(px(4.0))
            .cursor_pointer()
            .when(lit, |el| el.bg(hover))
            .hover(move |s| s.bg(hover))
            .child(crate::ui::profile_card::mark(&id, crate::ui::profile_card::Side::Right))
            .on_mouse_down(
                MouseButton::Right,
                self.right_click(MenuOf::Member { key: k.clone(), server: None, user_id: uid.clone() }, cx),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_dialog(Dialog::Profile { key: k.clone(), user_id: uid.clone(), server: None }, window, cx)
            }))
            .child(div().relative().flex_none().child(avatar(Some(&user), 40.0, p)).children(dot))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .min_w_0()
                            .items_baseline()
                            .gap(px(6.0))
                            .h(px(24.0))
                            .child(
                                div()
                                    .relative()
                                    .truncate()
                                    .text_base()
                                    .line_height(px(24.0))
                                    .font_weight(FontWeight::BOLD)
                                    .group_hover(group, |s| s.left(px(2.0)))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(format!("@{}", user.username)),
                            ),
                    )
                    .child(div().truncate().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(line)),
            );
        motion::rise(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                // The web's `border-t ... first:border-t-0` never shows: each line is first in its own box.
                .py(px(8.0))
                .child(who)
                .child(actions),
            SharedString::from(format!("friend-in|{id}")),
            Duration::from_millis(18 * n.min(12) as u64),
            6.0,
        )
        .into_any_element()
    }

    /// A friend's "more" menu: remove them, or block them.
    fn friend_more_menu(
        &mut self,
        key: &str,
        user_id: &str,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item =
            |id: &'static str, glyph: &'static str, label: String, danger: bool, act: Act, cx: &mut Context<Self>| {
                let (k, uid) = (key.to_owned(), user_id.to_owned());
                let (hover, fg, glyph_fg) = if danger {
                    (alpha(p.destructive, 0.1), p.destructive, p.destructive)
                } else {
                    (p.accent.into(), p.foreground, p.muted_foreground)
                };
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(radius_sm())
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(fg)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(icon(glyph).size(px(16.0)).text_color(glyph_fg))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.friends.more = None;
                        this.friend_act(&k, &uid, act, cx);
                    }))
            };
        let list = div()
            .id("friend-more-menu")
            .w(px(192.0))
            .rounded(radius_md())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(4.0))
            .shadow(crate::ui::profile_card::shadow_md())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.friends.more = None;
                cx.notify();
            }))
            .child(item("friend-more-remove", "user-minus", t("dms-calls.friends.remove"), false, Act::Remove, cx))
            .child(div().mx(px(-4.0)).my(px(4.0)).h(px(1.0)).bg(p.border))
            .child(item("friend-more-block", "ban", t("dms-calls.friends.page.block"), true, Act::Block, cx));
        gpui_kit::deferred(
            gpui_kit::anchored()
                .anchor(gpui_kit::Anchor::TopRight)
                .position(at)
                .snap_to_window_with_margin(gpui_kit::Edges::all(px(8.0)))
                .child(motion::rise(list, SharedString::from(format!("friend-more|{user_id}")), Duration::ZERO, -4.0)),
        )
        .with_priority(2)
        .into_any_element()
    }

    /// The friend buttons on someone's profile card (`FriendActions`): add
    /// them, take or turn down their request, cancel yours, unfriend, block;
    /// and the friends you share, when everyone involved allows it. None for
    /// you, agents, or an instance without friends.
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
        let name = self
            .core
            .shared
            .read(|s| s.instance(key).and_then(|i| i.users.get(user_id)).map(user_name))
            .unwrap_or_default();
        let relation = self.friends.relation.as_ref().filter(|(id, _)| id == user_id).map(|(_, r)| r.clone());
        let mutual = relation.as_ref().map(|r| r.mutual_friends.clone()).unwrap_or_default();
        let may_request = relation.as_ref().is_none_or(|r| r.may_request);
        let busy = self.friends.busy.contains(user_id);
        let emerald_bg = rgb(0x10b981);
        let emerald_fg: gpui_kit::Hsla = if p.dark { rgb(0x6ee7b7).into() } else { rgb(0x047857).into() };
        // `Action`: h-9 rounded-xl, muted unless it's good news.
        let button = |id: &'static str, glyph: &'static str, label: String, good: bool, disabled: bool| {
            let (bg, fg, hover): (gpui_kit::Hsla, gpui_kit::Hsla, gpui_kit::Hsla) = if good {
                (alpha(emerald_bg, 0.15), emerald_fg, alpha(emerald_bg, 0.25))
            } else {
                (p.muted.into(), p.foreground.into(), alpha(p.muted, 0.7))
            };
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .h(px(36.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .rounded(radius_xl())
                .bg(bg)
                .text_color(fg)
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .when(busy || disabled, |el| el.opacity(0.6))
                .when(!disabled, |el| el.cursor_pointer().hover(move |s| s.bg(hover)).active(|s| s.top(px(1.0))))
                .child(icon(glyph).size(px(16.0)))
                .child(div().truncate().child(label))
        };
        let on = |act: Act, cx: &mut Context<Self>| act_on(key, user_id, act, cx);
        let main = match state {
            0 => {
                let b = button(
                    "friend-add",
                    if busy { "loader-circle" } else { "user-plus" },
                    t("dms-calls.friends.add"),
                    false,
                    !may_request,
                );
                if may_request {
                    b.on_click(on(Act::Request, cx)).into_any_element()
                } else {
                    let tip = t_with("dms-calls.friends.actions.notTaking", &[("name", Arg::Str(&name))]);
                    b.tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                        .into_any_element()
                }
            }
            OUTGOING => swap(
                "friend-requested",
                "clock",
                t("dms-calls.friends.actions.requested"),
                "x",
                t("dms-calls.friends.actions.cancelRequest"),
                busy,
                p,
            )
            .on_click(on(Act::Remove, cx))
            .into_any_element(),
            INCOMING => div()
                .flex_1()
                .min_w_0()
                .flex()
                .gap(px(6.0))
                .child(
                    button("friend-accept", "check", t("dms-calls.friends.actions.accept"), true, false)
                        .on_click(on(Act::Accept, cx)),
                )
                .child(
                    button("friend-decline", "x", t("dms-calls.friends.actions.decline"), false, false)
                        .on_click(on(Act::Remove, cx)),
                )
                .into_any_element(),
            FRIEND => swap(
                "friend-friends",
                "user-check",
                t("dms-calls.friends.actions.friends"),
                "user-minus",
                t("dms-calls.friends.remove"),
                busy,
                p,
            )
            .on_click(on(Act::Remove, cx))
            .into_any_element(),
            _ => button("friend-unblock", "shield-off", t("dms-calls.friends.unblock"), false, false)
                .on_click(on(Act::Unblock, cx))
                .into_any_element(),
        };
        let mut row = div().flex().gap(px(6.0)).child(motion::rise(
            div().flex().flex_1().min_w_0().child(main),
            SharedString::from(format!("friend-state|{user_id}|{state}")),
            Duration::ZERO,
            6.0,
        ));
        if state != BLOCKED {
            let (red, soft) = (p.destructive, alpha(p.destructive, 0.1));
            let tip = t_with("dms-calls.friends.actions.blockTitle", &[("name", Arg::Str(&name))]);
            row = row.child(
                div()
                    .id("friend-block")
                    .size(px(36.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(radius_xl())
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .when(busy, |el| el.opacity(0.6))
                    .hover(move |s| s.bg(soft).text_color(red))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("ban").size(px(16.0)))
                    .tooltip(move |window, cx| crate::ui::overlay::Tip::new(tip.clone()).build(window, cx))
                    .on_click(on(Act::Block, cx)),
            );
        }
        let mut out = div().flex().flex_col().gap(px(6.0));
        if !mutual.is_empty() {
            let text = if mutual.len() == 1 {
                t_with("dms-calls.friends.actions.mutualOne", &[("name", Arg::Str(&user_name(&mutual[0])))])
            } else {
                t_with("dms-calls.friends.actions.mutual", &[("count", Arg::Num(mutual.len() as i64))])
            };
            out = out.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .pt(px(2.0))
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
                    .child(div().truncate().child(text)),
                SharedString::from(format!("mutual-{user_id}")),
                Duration::ZERO,
                4.0,
            ));
        }
        Some(out.child(row).into_any_element())
    }
}

thread_local! {
    /// Where each tab, and the row of them (`None`), was last drawn: the pill glides to the picked one.
    static TAB_PLACES: std::cell::RefCell<std::collections::HashMap<Option<Tab>, (f32, f32)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Where each friend's "more" button was drawn, for its menu.
    static MORE_PLACES: std::cell::RefCell<std::collections::HashMap<String, gpui_kit::Bounds<gpui_kit::Pixels>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn tab_key(tab: Tab) -> &'static str {
    match tab {
        Tab::Online => "dms-calls.friends.tab.online",
        Tab::All => "dms-calls.friends.tab.all",
        Tab::Pending => "dms-calls.friends.tab.pending",
        Tab::Blocked => "dms-calls.friends.tab.blocked",
    }
}

/// A field's border, lit while it has the keyboard (`focus-within:border-primary/60 focus-within:ring-2 focus-within:ring-primary/20`).
fn focus_ring(el: gpui_kit::Div, on: bool, p: &Palette) -> gpui_kit::Div {
    if !on {
        return el.border_color(p.border);
    }
    el.border_color(alpha(p.primary, 0.6)).shadow(vec![gpui_kit::BoxShadow {
        color: alpha(p.primary, 0.2),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(2.0),
        inset: false,
    }])
}

/// A tab with nobody in it (`Empty`): a floating tile, a title, a line, maybe a button.
fn empty(
    glyph: &'static str,
    title: &str,
    text: &str,
    action: Option<AnyElement>,
    p: &Palette,
    window: &Window,
) -> AnyElement {
    let tile = div()
        .size(px(64.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_3xl())
        .bg(alpha(p.primary, 0.15))
        .text_color(p.primary)
        .child(icon(glyph).size(px(28.0)));
    let tile =
        motion::ambient(div().child(tile), "friends-empty-float", Duration::from_millis(4000), window, |el, t| {
            el.mt(px(-4.0 * (t * std::f32::consts::TAU).sin().abs()))
        });
    motion::rise(
        div()
            .mx_auto()
            .max_w(px(384.0))
            .flex()
            .flex_col()
            .items_center()
            .px(px(24.0))
            .pt(px(64.0))
            .text_center()
            .child(div().h(px(64.0)).child(tile))
            .child(div().mt(px(16.0)).font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
            .child(
                div().mt(px(4.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(text.to_owned()),
            )
            .children(action),
        SharedString::from(format!("friends-empty|{title}")),
        Duration::ZERO,
        10.0,
    )
    .into_any_element()
}

/// Lines held in place while the list loads.
fn loading(p: &Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .px(px(4.0))
        .pt(px(8.0))
        .children((0..3).map(|_| {
            div().flex().items_center().gap(px(12.0)).child(div().size(px(40.0)).rounded_full().bg(p.muted)).child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().h(px(12.0)).w(gpui_kit::relative(1.0 / 3.0)).rounded(px(4.0)).bg(p.muted))
                    .child(div().h(px(10.0)).w(gpui_kit::relative(0.2)).rounded(px(4.0)).bg(p.muted)),
            )
        }))
        .into_any_element()
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

/// A button whose label slides away while hovered for what clicking it does
/// ("Friends" becomes "Remove friend"), in red.
fn swap(
    id: &'static str,
    glyph: &'static str,
    label: String,
    hover_glyph: &'static str,
    hover_label: String,
    busy: bool,
    p: &Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (red, soft) = (p.destructive, alpha(p.destructive, 0.1));
    div()
        .id(id)
        .group(id)
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(36.0))
        .overflow_hidden()
        .rounded(radius_xl())
        .bg(p.muted)
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .when(busy, |el| el.opacity(0.6))
        .hover(move |s| s.bg(soft))
        .active(|s| s.top(px(1.0)))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .h(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .text_color(p.foreground)
                .group_hover(id, |s| s.top(px(-24.0)).opacity(0.0))
                .child(icon(glyph).size(px(16.0)))
                .child(div().truncate().child(label)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(24.0))
                .h(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .opacity(0.0)
                .text_color(red)
                .group_hover(id, |s| s.top(px(0.0)).opacity(1.0))
                .child(icon(hover_glyph).size(px(16.0)))
                .child(div().truncate().child(hover_label)),
        )
}
