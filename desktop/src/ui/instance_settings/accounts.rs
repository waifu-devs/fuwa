//! The Accounts page: every account on the instance, for its admins to find
//! one, make it an admin, give it a new password, or turn it off so it can't
//! sign in. As in the web's `Accounts.tsx`, whose row menus are buttons here.

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, hsla, px, uniform_list,
};

use super::{InstanceSettingsEvent, InstanceSettingsView};
use crate::core::dms::now_ms;
use crate::core::instance_manage::{self as manage, REASON_MAX};
use crate::core::store::user_name;
use crate::pb::{self, AccountFilter as Filter};
use crate::ui::motion;
use crate::ui::server_settings::{amber, pill, shimmer_rows, spinner};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{
    app_badge, avatar, card, danger_button, error_line, icon, icon_button, icon_button_in, is_agent, pal,
    primary_button, soft_button,
};

/// A row's height, with the space under it; every row is as tall so the
/// list can skip what's out of sight.
const ROW: f32 = 84.0;
const GAP: f32 = 6.0;

/// What a dialog over the list is for.
#[derive(Clone)]
pub(super) enum Pending {
    TurnOff(pb::AccountSummary),
    Reset(pb::AccountSummary),
}

/// The page's own state; it changes accounts straight away, not through the save bar.
pub(super) struct Accounts {
    pub query: Entity<InputState>,
    /// What's searched for, once typing pauses.
    search: String,
    typed: u64,
    filter: Filter,
    list: Option<Vec<pb::AccountSummary>>,
    scroll: gpui_kit::UniformListScrollHandle,
    totals: Option<pb::AccountTotals>,
    has_more: bool,
    loading_more: bool,
    error: Option<String>,
    /// The account being changed from its row.
    busy: Option<String>,
    /// Bumped by each new search, so an older answer is dropped.
    request: u64,
    pub pending: Option<Pending>,
    reason: Entity<TextareaState>,
    two_factor: bool,
    password: Option<String>,
    revealed: bool,
    copied: Option<Instant>,
    dialog_busy: bool,
    dialog_error: Option<String>,
}

impl Accounts {
    pub fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search by name or username"));
        let reason = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 4));
        let search =
            cx.subscribe_in(&query, window, |this: &mut InstanceSettingsView, _, e: &InputEvent, window, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                // Search as you type, once typing pauses.
                this.accounts.typed += 1;
                let typed = this.accounts.typed;
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.accounts.typed != typed {
                            return;
                        }
                        let search = this.accounts.query.read(cx).value().trim().to_owned();
                        if search != this.accounts.search {
                            this.accounts.search = search;
                            this.load_accounts(window, cx);
                        }
                    });
                })
                .detach();
            });
        let capped =
            cx.subscribe_in(&reason, window, |_: &mut InstanceSettingsView, state, e: &InputEvent, window, cx| {
                if !matches!(e, InputEvent::Change) {
                    return;
                }
                cx.notify();
                let value = state.read(cx).value().to_string();
                if value.chars().count() > REASON_MAX {
                    let cut: String = value.chars().take(REASON_MAX).collect();
                    state.update(cx, |s, cx| s.set_value(cut, window, cx));
                }
            });
        (
            Self {
                query,
                search: String::new(),
                typed: 0,
                filter: Filter::Unspecified,
                list: None,
                scroll: gpui_kit::UniformListScrollHandle::new(),
                totals: None,
                has_more: false,
                loading_more: false,
                error: None,
                busy: None,
                request: 0,
                pending: None,
                reason,
                two_factor: false,
                password: None,
                revealed: false,
                copied: None,
                dialog_busy: false,
                dialog_error: None,
            },
            vec![search, capped],
        )
    }
}

fn id_of(a: &pb::AccountSummary) -> String {
    a.user.as_ref().map(|u| u.id.clone()).unwrap_or_default()
}

fn name_of(a: &pb::AccountSummary) -> String {
    a.user.as_ref().map(user_name).unwrap_or_else(|| "Someone".into())
}

fn kind(a: &pb::AccountSummary) -> pb::AccountKind {
    a.user.as_ref().map_or(pb::AccountKind::Unspecified, |u| u.kind())
}

fn plural(n: i32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl InstanceSettingsView {
    /// Reads the first page again for the search and filter.
    pub(super) fn load_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts.request += 1;
        let n = self.accounts.request;
        self.accounts.error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        let (query, filter) = (self.accounts.search.clone(), self.accounts.filter);
        self.run(
            window,
            cx,
            async move { core.list_accounts(&key, query, filter, String::new()).await },
            move |this, result, _, cx| {
                if n != this.accounts.request {
                    return;
                }
                match result {
                    Ok(res) => this.take_accounts(res, false),
                    Err(problem) => this.accounts.error = Some(problem.message),
                }
                cx.notify();
            },
        );
    }

    fn take_accounts(&mut self, res: pb::ListAccountsResponse, more: bool) {
        let a = &mut self.accounts;
        match (&mut a.list, more) {
            (Some(list), true) => list.extend(res.accounts),
            _ => {
                a.list = Some(res.accounts);
                a.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
            }
        }
        a.has_more = res.has_more;
        if res.totals.is_some() {
            a.totals = res.totals;
        }
    }

    fn more_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = self.accounts.list.as_ref().and_then(|l| l.last()).map(id_of) else { return };
        if self.accounts.loading_more {
            return;
        }
        self.accounts.loading_more = true;
        let n = self.accounts.request;
        let (core, key) = (self.core.clone(), self.key.clone());
        let (query, filter) = (self.accounts.search.clone(), self.accounts.filter);
        self.run(
            window,
            cx,
            async move { core.list_accounts(&key, query, filter, last).await },
            move |this, result, _, cx| {
                this.accounts.loading_more = false;
                if n != this.accounts.request {
                    return;
                }
                match result {
                    Ok(res) => this.take_accounts(res, true),
                    Err(problem) => this.toast("circle-alert", problem.message, cx),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn toast(&self, icon: &'static str, title: String, cx: &mut Context<Self>) {
        cx.emit(InstanceSettingsEvent::Toast { icon, title });
    }

    /// Puts an account's new state on the page, and keeps the counts in step.
    fn replace_account(&mut self, next: pb::AccountSummary) {
        let id = id_of(&next);
        let filter = self.accounts.filter;
        let a = &mut self.accounts;
        if let (Some(list), Some(totals)) = (&a.list, &mut a.totals)
            && let Some(prev) = list.iter().find(|x| id_of(x) == id)
        {
            manage::retotal(totals, prev, &next);
        }
        if let Some(list) = &mut a.list {
            for x in list.iter_mut() {
                if id_of(x) == id {
                    *x = next.clone();
                }
            }
            list.retain(|x| manage::fits(x, filter));
        }
    }

    fn change_account(
        &mut self,
        account: &pb::AccountSummary,
        admin: Option<bool>,
        disabled: Option<bool>,
        done: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id_of(account);
        self.accounts.busy = Some(id.clone());
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(
            window,
            cx,
            async move { core.update_account(&key, id, admin, disabled, String::new()).await },
            move |this, result, _, cx| {
                this.accounts.busy = None;
                match result {
                    Ok(next) => {
                        this.replace_account(next);
                        this.toast("check", done, cx);
                    }
                    Err(problem) => this.toast("circle-alert", problem.message, cx),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn ask(&mut self, pending: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let a = &mut self.accounts;
        a.pending = Some(pending);
        a.two_factor = false;
        a.password = None;
        a.revealed = false;
        a.copied = None;
        a.dialog_busy = false;
        a.dialog_error = None;
        let reason = a.reason.clone();
        reason.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// Closes the dialog over the list, if one's open.
    pub(super) fn close_account_dialog(&mut self, cx: &mut Context<Self>) -> bool {
        if self.accounts.pending.take().is_none() {
            return false;
        }
        self.accounts.password = None;
        cx.notify();
        true
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.accounts.pending.clone() else { return };
        if self.accounts.dialog_busy {
            return;
        }
        self.accounts.dialog_busy = true;
        self.accounts.dialog_error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        match pending {
            Pending::TurnOff(account) => {
                let id = id_of(&account);
                let name = name_of(&account);
                let reason = self.accounts.reason.read(cx).value().trim().to_owned();
                // Turned off, they stop being an admin too.
                let admin = account.admin.then_some(false);
                self.run(
                    window,
                    cx,
                    async move { core.update_account(&key, id, admin, Some(true), reason).await },
                    move |this, result, _, cx| {
                        this.accounts.dialog_busy = false;
                        match result {
                            Ok(next) => {
                                this.replace_account(next);
                                this.accounts.pending = None;
                                this.toast("power-off", format!("{name} is turned off"), cx);
                            }
                            Err(problem) => this.accounts.dialog_error = Some(problem.message),
                        }
                        cx.notify();
                    },
                );
            }
            Pending::Reset(account) => {
                let id = id_of(&account);
                let two_factor = self.accounts.two_factor;
                self.run(
                    window,
                    cx,
                    async move { core.reset_account_password(&key, id, two_factor).await },
                    move |this, result, _, cx| {
                        this.accounts.dialog_busy = false;
                        match result {
                            Ok(password) => this.accounts.password = Some(password),
                            Err(problem) => this.accounts.dialog_error = Some(problem.message),
                        }
                        cx.notify();
                    },
                );
            }
        }
        cx.notify();
    }

    pub(super) fn accounts_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.accounts.list.is_none() && self.accounts.error.is_none() && self.accounts.request == 0 {
            self.load_accounts(window, cx);
        }
        let now = now_ms();
        let me = self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        let streamer = self.core.prefs().streamer_mode;
        let a = &self.accounts;
        let count = |n: Option<i64>| n.map(|n| format!(" · {n}")).unwrap_or_default();
        let totals = a.totals;
        let tabs = [
            (Filter::Unspecified, format!("All{}", count(totals.as_ref().map(|t| t.all)))),
            (Filter::Admins, format!("Admins{}", count(totals.as_ref().map(|t| t.admins)))),
            (Filter::Disabled, format!("Turned off{}", count(totals.as_ref().map(|t| t.disabled)))),
        ];
        let filter = a.filter;
        let segmented = segmented(&tabs, filter, p, window, cx);
        let top = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&a.query).prefix(icon("search").size(px(15.0)).text_color(p.muted_foreground))),
            )
            .child(segmented);

        let body: AnyElement = if let Some(error) = &a.error {
            div().text_sm().text_color(p.muted_foreground).child(error.clone()).into_any_element()
        } else if let Some(list) = a.list.as_ref() {
            let shown = list.len();
            let more = a.has_more;
            let header = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(icon("users").size(px(14.0)))
                .child(format!(
                    "{shown}{} {}",
                    if more { "+" } else { "" },
                    if shown == 1 { "ACCOUNT" } else { "ACCOUNTS" }
                ));
            let column = div().flex_1().min_h_0().flex().flex_col().gap(px(10.0)).child(header);
            if shown == 0 {
                column
                    .child(motion::rise(
                        div().py(px(32.0)).flex().justify_center().text_sm().text_color(p.muted_foreground).child(
                            if !a.search.is_empty() {
                                "Nobody matches that."
                            } else if filter == Filter::Disabled {
                                "No account is turned off."
                            } else {
                                "No accounts here."
                            },
                        ),
                        SharedString::from(format!("accounts-none-{filter:?}")),
                        Duration::ZERO,
                        8.0,
                    ))
                    .into_any_element()
            } else {
                // Only the rows in sight are drawn, so a long list stays quick;
                // "Show more" is the last row when there are more.
                let rows = uniform_list(
                    "account-rows",
                    shown + usize::from(more),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        let p = pal(cx);
                        range
                            .map(|n| {
                                let account = this.accounts.list.as_ref().and_then(|l| l.get(n)).cloned();
                                let el = match account {
                                    Some(account) => {
                                        let mine = me.as_deref() == Some(id_of(&account).as_str());
                                        this.account_row(&account, n, mine, streamer, now, &p, window, cx)
                                    }
                                    None => {
                                        let loading = this.accounts.loading_more;
                                        div()
                                            .size_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                soft_button(
                                                    "accounts-more",
                                                    if loading { "Loading…" } else { "Show more" },
                                                    &p,
                                                )
                                                .on_click(
                                                    cx.listener(|this, _, window, cx| this.more_accounts(window, cx)),
                                                ),
                                            )
                                            .into_any_element()
                                    }
                                };
                                div().h(px(ROW)).pb(px(GAP)).child(el).into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&a.scroll)
                .flex_1()
                .min_h_0();
                column.child(rows).into_any_element()
            }
        } else {
            shimmer_rows(4, p).into_any_element()
        };
        div().flex_1().min_h_0().flex().flex_col().gap(px(16.0)).child(top).child(body).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn account_row(
        &self,
        a: &pb::AccountSummary,
        index: usize,
        me: bool,
        streamer: bool,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = id_of(a);
        let name = name_of(a);
        let user = a.user.clone();
        let local = kind(a) == pb::AccountKind::Local;
        let agent = is_agent(user.as_ref());
        let busy = self.accounts.busy.as_deref() == Some(id.as_str());
        let joined = a
            .created_at
            .as_ref()
            .map(|t| manage::day_label(t.seconds * 1000, now))
            .map(|d| if d == "Today" || d == "Yesterday" { d.to_lowercase() } else { d })
            .unwrap_or_default();
        let active = a
            .last_seen_at
            .as_ref()
            .map(|t| manage::active_ago(t.seconds * 1000, now).replacen("Active", "active", 1))
            .unwrap_or_default();
        let username = user.as_ref().map(|u| u.username.clone()).unwrap_or_default();
        let handle = if me && streamer { "@you".to_owned() } else { format!("@{username}") };
        let mut facts = vec![plural(a.servers, "server", "servers")];
        if a.servers_owned > 0 {
            facts.push(format!("owns {}", a.servers_owned));
        }
        facts.push(format!("{} signed in", plural(a.sessions, "device", "devices")));
        let destructive = p.destructive;
        let green = hsla(0.42, 0.65, 0.45, 1.0);

        let mut badges = div().flex().items_center().gap(px(6.0)).min_w_0();
        badges = badges
            .child(
                div()
                    .min_w_0()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::BOLD)
                    .text_color(crate::ui::widgets::hue_color(&id, p.dark))
                    .when(a.disabled, |el| el.line_through().opacity(0.8))
                    .child(name.clone()),
            )
            .when(me, |el| el.child(pill("YOU", p.muted_foreground.into())))
            .when(a.admin, |el| {
                el.child(motion::once(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .px(px(7.0))
                        .h(px(18.0))
                        .rounded_full()
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .text_size(px(10.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(icon("shield").size(px(11.0)))
                        .child("ADMIN"),
                    SharedString::from(format!("account-admin-{id}")),
                    Duration::from_millis(360),
                    |el, t| el.opacity(t),
                ))
            })
            .when(a.disabled, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .px(px(7.0))
                        .h(px(18.0))
                        .rounded_full()
                        .bg(alpha(destructive, 0.15))
                        .text_color(destructive)
                        .text_size(px(10.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(icon("power-off").size(px(11.0)))
                        .child("OFF"),
                )
            })
            .when(a.two_factor, |el| el.child(icon("shield-check").size(px(14.0)).text_color(green)))
            .when(agent, |el| el.child(app_badge(SharedString::from(format!("account-agent-{id}")), "AGENT", p)))
            .when(kind(a) == pb::AccountKind::Linked, |el| {
                el.child(icon("link-2").size(px(14.0)).text_color(p.muted_foreground))
            })
            .when(kind(a) == pb::AccountKind::Sso, |el| {
                el.child(icon("building").size(px(14.0)).text_color(p.muted_foreground))
            });

        // What can be done to them, where the web has a menu. Not to yourself.
        let mut actions = div().flex().items_center().gap(px(2.0)).flex_none();
        if busy {
            actions = actions.child(div().size(px(32.0)).flex().items_center().justify_center().child(spinner(
                SharedString::from(format!("account-busy-{id}")),
                16.0,
                window,
            )));
        } else if !me {
            let button = |glyph: &str, tip: &'static str, color: Option<gpui_kit::Rgba>| {
                let el = match color {
                    Some(c) => icon_button_in(SharedString::from(format!("account-{glyph}-{id}")), glyph, p, c),
                    None => icon_button(SharedString::from(format!("account-{glyph}-{id}")), glyph, p),
                };
                el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
            };
            if !a.admin && !a.disabled && !agent {
                let (acc, n) = (a.clone(), name.clone());
                actions = actions.child(button("shield", "Make instance admin", Some(p.primary)).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.change_account(&acc, Some(true), None, format!("{n} is an instance admin now"), window, cx)
                    }),
                ));
            }
            if a.admin {
                let (acc, n) = (a.clone(), name.clone());
                actions = actions.child(button("shield-off", "Remove admin", None).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.change_account(&acc, Some(false), None, format!("{n} isn't an admin anymore"), window, cx)
                    },
                )));
            }
            if local {
                let acc = a.clone();
                actions = actions.child(button("key-round", "Reset password", Some(p.primary)).on_click(
                    cx.listener(move |this, _, window, cx| this.ask(Pending::Reset(acc.clone()), window, cx)),
                ));
            }
            if a.disabled {
                let (acc, n) = (a.clone(), name.clone());
                actions = actions.child(button("power", "Turn back on", Some(green.to_rgb())).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.change_account(&acc, None, Some(false), format!("{n} can sign in again"), window, cx)
                    },
                )));
            } else {
                let acc = a.clone();
                actions = actions.child(button("power-off", "Turn off", Some(p.destructive)).on_click(
                    cx.listener(move |this, _, window, cx| this.ask(Pending::TurnOff(acc.clone()), window, cx)),
                ));
            }
        }

        let off_line = a.disabled.then(|| {
            let when = a.disabled_at.as_ref().map(|t| manage::day_label(t.seconds * 1000, now).to_lowercase());
            let why = if a.disabled_reason.is_empty() {
                ", no reason given.".to_owned()
            } else {
                format!(": {}", a.disabled_reason)
            };
            format!("Turned off {}{why}", when.unwrap_or_default())
        });
        let tint: Hsla = if a.disabled { alpha(destructive, 0.05) } else { p.card.into() };
        let edge: Hsla = if a.disabled { alpha(destructive, 0.3) } else { p.border.into() };
        let hover_edge: Hsla = if a.disabled { alpha(destructive, 0.45) } else { alpha(p.primary, 0.3) };
        let row = div()
            .id(SharedString::from(format!("account-{id}")))
            .h_full()
            .flex()
            .flex_col()
            .justify_center()
            .p(px(10.0))
            .rounded(corner(16.0))
            .bg(tint)
            .border_1()
            .border_color(edge)
            .hover(move |s| s.border_color(hover_edge))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(div().flex_none().when(a.disabled, |el| el.opacity(0.5)).child(avatar(
                        user.as_ref(),
                        40.0,
                        p,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(badges)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(format!("{handle} · joined {joined} · {active}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(match off_line {
                                        Some(_) => alpha(destructive, 0.9),
                                        None => alpha(p.muted_foreground, 0.8),
                                    })
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(off_line.unwrap_or_else(|| facts.join(" · "))),
                            ),
                    )
                    .child(actions),
            );
        motion::rise(
            row,
            SharedString::from(format!("account-in-{id}")),
            Duration::from_millis(20 * index.min(14) as u64),
            10.0,
        )
        .into_any_element()
    }

    /// The dialog over the list: turning someone off, or a new password.
    pub(super) fn account_dialog(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pending = self.accounts.pending.clone()?;
        let a = &self.accounts;
        let busy = a.dialog_busy;
        let (account, danger) = match &pending {
            Pending::TurnOff(account) => (account.clone(), true),
            Pending::Reset(account) => (account.clone(), false),
        };
        let name = name_of(&account);
        let who = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(corner(16.0))
            .bg(alpha(p.muted, 0.6))
            .child(avatar(account.user.as_ref(), 40.0, p))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::BOLD).text_ellipsis().child(name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", account.user.as_ref().map(|u| u.username.as_str()).unwrap_or(""))),
                    ),
            );
        let (glyph, title, body, content, action): (&str, String, &str, AnyElement, Option<&str>) =
            match (&pending, &a.password) {
                (Pending::Reset(_), Some(password)) => (
                    "key-round",
                    format!("{name}'s new password"),
                    "Send it to them privately. It's shown only this once; they can change it in their account \
                     settings after signing in.",
                    self.new_password(password, p, window, cx),
                    None,
                ),
                (Pending::Reset(_), None) => {
                    let two = a.two_factor;
                    let mut content = div().flex().flex_col().gap(px(12.0)).child(who);
                    if account.two_factor {
                        content = content.child(
                            div()
                                .id("reset-two-factor")
                                .flex()
                                .items_start()
                                .gap(px(12.0))
                                .p(px(12.0))
                                .rounded(corner(12.0))
                                .border_1()
                                .border_color(p.border)
                                .cursor_pointer()
                                .hover({
                                    let c = alpha(p.primary, 0.3);
                                    move |s| s.border_color(c)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.accounts.two_factor = !two;
                                    cx.notify();
                                }))
                                .child(Switch::new("reset-two-factor-switch").checked(two).on_change({
                                    let entity = cx.entity().downgrade();
                                    move |on, _, cx| {
                                        let on = *on;
                                        let _ = entity.update(cx, |this, cx| {
                                            this.accounts.two_factor = on;
                                            cx.notify();
                                        });
                                    }
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .child("Also turn off two-step sign-in"),
                                        )
                                        .child(div().text_xs().text_color(p.muted_foreground).child(
                                            "For someone who lost their authenticator app and their backup codes.",
                                        )),
                                ),
                        );
                    }
                    (
                        "key-round",
                        format!("Reset {name}'s password"),
                        "They get a new random password and are signed out everywhere. Their old password stops \
                         working.",
                        content.into_any_element(),
                        Some(if busy { "Resetting…" } else { "Reset password" }),
                    )
                }
                (Pending::TurnOff(_), _) => {
                    let content = div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(who)
                        .when(account.admin, |el| {
                            el.child(
                                div()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(corner(12.0))
                                    .bg(amber(p).opacity(0.1))
                                    .text_sm()
                                    .text_color(amber(p))
                                    .child("They stop being an instance admin too."),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .child(div().text_sm().font_weight(FontWeight::BOLD).child("Reason"))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.muted_foreground)
                                                .child("Optional, only admins see it"),
                                        ),
                                )
                                .child(Textarea::new(&a.reason)),
                        );
                    (
                        "power-off",
                        format!("Turn off {name}"),
                        "They're signed out on every device and can't sign in until an admin turns the account back \
                         on. Their messages and servers stay.",
                        content.into_any_element(),
                        Some(if busy { "Turning off…" } else { "Turn off" }),
                    )
                }
            };
        let shown = a.password.is_some();
        let panel = card(p)
            .w(px(460.0))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(14.0))
                    .child(
                        div()
                            .size(px(44.0))
                            .flex_none()
                            .rounded(corner(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(if danger { p.destructive } else { p.primary }, 0.14))
                            .text_color(if danger { p.destructive } else { p.primary })
                            .child(icon(glyph).size(px(22.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(title))
                            .child(div().text_sm().text_color(p.muted_foreground).child(body)),
                    ),
            )
            .child(content)
            .when(shown && a.two_factor, |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child("Two-step sign-in is off for them now. They can set it up again after signing in."),
                )
            })
            .when_some(error_line(a.dialog_error.as_deref(), p), |el, e| el.child(e))
            .child(div().flex().justify_end().gap(px(10.0)).map(|el| {
                if shown {
                    el.child(primary_button("account-dialog-done", "Done", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.close_account_dialog(cx);
                        },
                    )))
                } else {
                    el.child(soft_button("account-dialog-cancel", "Cancel", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.close_account_dialog(cx);
                        },
                    )))
                    .when_some(action, |el, label| {
                        let button = if danger {
                            danger_button("account-dialog-ok", label, p)
                        } else {
                            primary_button("account-dialog-ok", label, p)
                        };
                        el.child(
                            button
                                .when(busy, |el| el.opacity(0.7))
                                .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                        )
                    })
                }
            }));
        let tag = match (&pending, shown) {
            (Pending::TurnOff(_), _) => "off",
            (Pending::Reset(_), false) => "reset",
            (Pending::Reset(_), true) => "password",
        };
        Some(
            motion::fade_in(
                crate::ui::overlay::scrim("account-scrim", p)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.accounts.dialog_busy {
                            this.close_account_dialog(cx);
                        }
                    }))
                    .child(motion::rise(
                        div().id("account-panel").on_click(|_, _, cx| cx.stop_propagation()).child(panel),
                        SharedString::from(format!("account-dialog-{tag}-{}", id_of(&account))),
                        Duration::ZERO,
                        24.0,
                    )),
                SharedString::from(format!("account-dialog-fade-{}", id_of(&account))),
                Duration::from_millis(180),
            )
            .into_any_element(),
        )
    }

    /// The password, typed out letter by letter, behind a veil in streamer mode.
    fn new_password(&self, password: &str, p: &Palette, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let hidden = self.core.prefs().streamer_mode && !self.accounts.revealed;
        let copied = self.accounts.copied.is_some_and(|at| at.elapsed() < Duration::from_millis(1600));
        let mut letters =
            div().flex().justify_center().font_family("monospace").text_lg().font_weight(FontWeight::BOLD);
        for (n, ch) in password.chars().enumerate() {
            letters = letters.child(motion::rise(
                div().when(ch == '-', |el| el.text_color(p.muted_foreground)).child(ch.to_string()),
                SharedString::from(format!("password-letter-{n}")),
                Duration::from_millis(100 + 25 * n as u64),
                8.0,
            ));
        }
        let code = div()
            .relative()
            .flex_1()
            .min_w_0()
            .px(px(12.0))
            .py(px(12.0))
            .rounded(corner(12.0))
            .bg(p.muted)
            .child(if hidden {
                div()
                    .flex()
                    .justify_center()
                    .font_family("monospace")
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(alpha(p.muted_foreground, 0.4))
                    .child("•".repeat(password.chars().count()))
                    .into_any_element()
            } else {
                letters.into_any_element()
            })
            .when(hidden, |el| {
                el.child(
                    div()
                        .id("password-reveal")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .rounded(corner(12.0))
                        .bg(alpha(p.background, 0.6))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.accounts.revealed = true;
                            cx.notify();
                        }))
                        .child(icon("eye").size(px(16.0)))
                        .child("Hidden by streamer mode. Show it anyway"),
                )
            });
        let text = password.to_owned();
        let copy = div()
            .id("password-copy")
            .size(px(48.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(corner(12.0))
            .border_1()
            .border_color(p.border)
            .cursor_pointer()
            .hover({
                let c = alpha(p.primary, 0.08);
                move |s| s.bg(c)
            })
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                this.accounts.copied = Some(Instant::now());
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(1700)).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
                cx.notify();
            }))
            .child(motion::once(
                div().child(icon(if copied { "check" } else { "copy" }).size(px(16.0)).text_color(if copied {
                    hsla(0.42, 0.65, 0.45, 1.0)
                } else {
                    p.foreground.into()
                })),
                SharedString::from(format!("password-copied-{copied}")),
                Duration::from_millis(300),
                |el, t| el.opacity(t),
            ));
        div().flex().items_center().gap(px(8.0)).child(code).child(copy).into_any_element()
    }
}

/// A row of choices with a highlight that glides to the picked one.
fn segmented(
    tabs: &[(Filter, String)],
    value: Filter,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<InstanceSettingsView>,
) -> AnyElement {
    // Each choice is as wide as its words, so the highlight measures them the same way.
    let widths: Vec<f32> = tabs.iter().map(|(_, label)| 24.0 + 7.4 * label.chars().count() as f32).collect();
    let at = tabs.iter().position(|(f, _)| *f == value).unwrap_or(0);
    let left: f32 = widths[..at].iter().sum::<f32>() + 4.0;
    let x = motion::follow("accounts-filter-x", left, window, cx);
    let w = motion::follow("accounts-filter-w", widths[at], window, cx);
    let mut row = div()
        .relative()
        .flex()
        .h(px(40.0))
        .p(px(4.0))
        .rounded(corner(12.0))
        .bg(p.secondary)
        .child(div().absolute().top(px(4.0)).bottom(px(4.0)).left(px(x)).w(px(w)).rounded(corner(9.0)).bg(p.card));
    for ((filter, label), width) in tabs.iter().zip(widths) {
        let (filter, on) = (*filter, *filter == value);
        row = row.child(
            div()
                .id(SharedString::from(format!("accounts-filter-{filter:?}")))
                .relative()
                .w(px(width))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .cursor_pointer()
                .text_color(if on { p.foreground } else { p.muted_foreground })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.accounts.filter != filter {
                        this.accounts.filter = filter;
                        this.load_accounts(window, cx);
                        cx.notify();
                    }
                }))
                .child(label.clone()),
        );
    }
    row.into_any_element()
}
