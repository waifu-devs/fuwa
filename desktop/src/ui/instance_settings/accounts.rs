//! The Accounts page: every account on the instance, for its admins to find
//! one, make it an admin, give it a new password, or turn it off so it can't
//! sign in. As in the web's `Accounts.tsx`, whose row menus are buttons here.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, px, uniform_list,
};

use super::controls::{amber_text, area_box, dialog, dialog_buttons, input_box, segmented, shimmer, switch};
use super::{InstanceSettingsEvent, InstanceSettingsView};
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_manage::{self as manage, REASON_MAX};
use crate::core::store::user_name;
use crate::pb::{self, AccountFilter as Filter};
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_md, radius_sm, radius_xl};
use crate::ui::widgets::{app_badge, avatar, icon, is_agent, name_tint, pal};

/// `shadow-md`, under menus.
fn shadow_md() -> Vec<gpui_kit::BoxShadow> {
    let shadow = |y: f32, blur: f32, spread: f32| gpui_kit::BoxShadow {
        color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.1),
        offset: gpui_kit::point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    };
    vec![shadow(4.0, 6.0, -1.0), shadow(2.0, 4.0, -2.0)]
}

/// A row's height, with the space under it; every row is as tall so the
/// list can skip what's out of sight.
const ROW: f32 = 78.0;
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
    /// The row whose "…" menu is open, and where its button was pressed.
    menu: Option<(String, gpui_kit::Point<gpui_kit::Pixels>)>,
}

/// What a menu item does when picked.
type Action = Box<dyn Fn(&mut InstanceSettingsView, &mut Window, &mut Context<InstanceSettingsView>)>;

impl Accounts {
    pub fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t("instancesettings.accounts.search")));
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
                menu: None,
            },
            vec![search, capped],
        )
    }
}

fn id_of(a: &pb::AccountSummary) -> String {
    a.user.as_ref().map(|u| u.id.clone()).unwrap_or_default()
}

fn name_of(a: &pb::AccountSummary) -> String {
    a.user.as_ref().map(user_name).unwrap_or_else(|| t("common.someone"))
}

fn kind(a: &pb::AccountSummary) -> pb::AccountKind {
    a.user.as_ref().map_or(pb::AccountKind::Unspecified, |u| u.kind())
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

    pub(super) fn toast(&self, icon: &'static str, title: String, cx: &mut Context<Self>) {
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
                                this.toast(
                                    "power-off",
                                    t_with("instancesettings.accounts.turnedOff", &[("name", Arg::Str(&name))]),
                                    cx,
                                );
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
        let developer = self.core.prefs().developer_mode;
        let a = &self.accounts;
        // A filter's name, with how many accounts it has once that's known.
        let option = |bare: &str, counted: &str, n: Option<i64>| match n {
            Some(n) => t_with(counted, &[("count", Arg::Num(n))]),
            None => t(bare),
        };
        let totals = a.totals;
        let labels = vec![
            option("instancesettings.accounts.all", "instancesettings.accounts.allCount", totals.map(|t| t.all)),
            option(
                "instancesettings.accounts.admins",
                "instancesettings.accounts.adminsCount",
                totals.map(|t| t.admins),
            ),
            option("instancesettings.accounts.off", "instancesettings.accounts.offCount", totals.map(|t| t.disabled)),
        ];
        const FILTERS: [Filter; 3] = [Filter::Unspecified, Filter::Admins, Filter::Disabled];
        let filter = a.filter;
        let chosen = FILTERS.iter().position(|f| *f == filter).unwrap_or(0);
        let picker = segmented("accounts-filter", labels, chosen, p, window, cx, |this, n, window, cx| {
            let next = FILTERS[n];
            if this.accounts.filter != next {
                this.accounts.filter = next;
                this.accounts.menu = None;
                this.load_accounts(window, cx);
                cx.notify();
            }
        });
        let top = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().flex_1().min_w_0().child(focus_ring(
                input_box(Input::new(&a.query).appearance(false), Some("search"), p),
                has_focus(&a.query, window, cx),
                p,
            )))
            .child(picker);

        let body: AnyElement = if let Some(error) = &a.error {
            div().text_sm().text_color(p.muted_foreground).child(capitalized(error)).into_any_element()
        } else if let Some(list) = a.list.as_ref() {
            let shown = list.len();
            let more = a.has_more;
            // `flex items-center gap-1.5 text-xs font-bold tracking-wide uppercase`.
            let header = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(icon("users").size(px(14.0)))
                .child(
                    t_with(
                        if more { "instancesettings.accounts.countMore" } else { "instancesettings.accounts.count" },
                        &[("count", Arg::Num(shown as i64))],
                    )
                    .to_uppercase(),
                );
            let column = div().flex_1().min_h_0().flex().flex_col().gap(px(16.0)).child(header);
            if shown == 0 {
                column
                    .child(motion::rise(
                        div().py(px(32.0)).flex().justify_center().text_sm().text_color(p.muted_foreground).child(
                            if !a.search.is_empty() {
                                t("serversettings.shared.nobodyMatches")
                            } else if filter == Filter::Disabled {
                                t("instancesettings.accounts.noneOff")
                            } else {
                                t("instancesettings.accounts.none")
                            },
                        ),
                        SharedString::from(format!("accounts-none-{filter:?}")),
                        Duration::ZERO,
                        8.0,
                    ))
                    .into_any_element()
            } else {
                // Only the rows in sight are drawn, so a long list stays quick; "Show more" is the
                // last row when there are more.
                let rows = uniform_list(
                    "account-rows",
                    shown + usize::from(more),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        let p = pal(cx);
                        range
                            .map(|n| {
                                let account = this.accounts.list.as_ref().and_then(|l| l.get(n)).cloned();
                                match account {
                                    Some(account) => {
                                        let mine = me.as_deref() == Some(id_of(&account).as_str());
                                        let el = this
                                            .account_row(&account, n, mine, streamer, developer, now, &p, window, cx);
                                        div().h(px(ROW + GAP)).pb(px(GAP)).child(el).into_any_element()
                                    }
                                    None => {
                                        let loading = this.accounts.loading_more;
                                        div()
                                            .h(px(ROW + GAP))
                                            .pt(px(10.0))
                                            .flex()
                                            .justify_center()
                                            .child(
                                                button(
                                                    "accounts-more",
                                                    t("instancesettings.accounts.showMore"),
                                                    None,
                                                    Look::Outline,
                                                    false,
                                                    &p,
                                                )
                                                .rounded(radius_xl())
                                                .when(loading, |el| {
                                                    el.opacity(0.5).child(spinner("accounts-more-spin", 16.0, window))
                                                })
                                                .on_click(
                                                    cx.listener(|this, _, window, cx| this.more_accounts(window, cx)),
                                                ),
                                            )
                                            .into_any_element()
                                    }
                                }
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
            div().flex().flex_col().gap(px(6.0)).children((0..4).map(|n| shimmer(n, 64.0, p))).into_any_element()
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
        developer: bool,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = id_of(a);
        let name = name_of(a);
        let user = a.user.clone();
        let busy = self.accounts.busy.as_deref() == Some(id.as_str());
        let open = self.accounts.menu.as_ref().is_some_and(|(m, _)| *m == id);
        // "joined today · active 5 minutes ago", today and yesterday in the middle of a sentence.
        let joined = a
            .created_at
            .as_ref()
            .map(|t| manage::day_label(t.seconds * 1000, now))
            .map(|d| {
                if d == "Today" {
                    t("common.time.today")
                } else if d == "Yesterday" {
                    t("common.time.yesterday")
                } else {
                    d
                }
            })
            .map(|d| if d == t("common.time.today") || d == t("common.time.yesterday") { d.to_lowercase() } else { d })
            .unwrap_or_default();
        let seen = a
            .last_seen_at
            .as_ref()
            .map(|t| manage::active_ago(t.seconds * 1000, now))
            .filter(|s| s != "Active now")
            .map(|s| s.trim_start_matches("Active ").to_owned());
        let username = user.as_ref().map(|u| u.username.clone()).unwrap_or_default();
        let handle = if me && streamer { "@•••".to_owned() } else { format!("@{username}") };
        let when = match &seen {
            Some(when) => {
                t_with("instancesettings.accounts.joinedSeen", &[("day", Arg::Str(&joined)), ("when", Arg::Str(when))])
            }
            None => t_with("instancesettings.accounts.joinedNow", &[("day", Arg::Str(&joined))]),
        };
        let mut facts = vec![t_with("instancesettings.accounts.servers", &[("count", Arg::Num(i64::from(a.servers)))])];
        if a.servers_owned > 0 {
            facts.push(t_with("instancesettings.accounts.owns", &[("count", Arg::Num(i64::from(a.servers_owned)))]));
        }
        facts.push(t_with("instancesettings.accounts.devices", &[("count", Arg::Num(i64::from(a.sessions)))]));
        let destructive = p.destructive;
        let small_pill = |glyph: Option<&'static str>, text: String, fg: Hsla, bg: Hsla| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(2.0))
                .px(px(6.0))
                .py(px(1.0))
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .text_size(px(10.4))
                .line_height(px(14.0))
                .font_weight(FontWeight::BOLD)
                .when_some(glyph, |el, g| el.child(icon(g).size(px(12.0))))
                .child(text.to_uppercase())
        };

        // The name, and beside it: you, admin, off, two-step sign-in, and how it signs in.
        let badges = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .child(
                div()
                    .min_w_0()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .line_height(px(24.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(name_tint(&id, p))
                    .when(a.disabled, |el| el.line_through())
                    .child(name.clone()),
            )
            .when(me, |el| {
                el.child(small_pill(None, t("serversettings.shared.you"), p.muted_foreground.into(), p.muted.into()))
            })
            .when(a.admin, |el| {
                el.child(motion::once(
                    small_pill(
                        Some("shield"),
                        t("instancesettings.accounts.admin"),
                        p.primary.into(),
                        alpha(p.primary, 0.15),
                    ),
                    SharedString::from(format!("account-admin-{id}")),
                    Duration::from_millis(360),
                    |el, t| el.opacity(t),
                ))
            })
            .when(a.disabled, |el| {
                el.child(small_pill(
                    Some("power-off"),
                    t("serversettings.shared.off"),
                    destructive.into(),
                    alpha(destructive, 0.15),
                ))
            })
            .when(a.two_factor, |el| el.child(icon("shield-check").size(px(14.0)).text_color(gpui_kit::rgb(0x00bc7d))))
            .when(is_agent(user.as_ref()), |el| {
                el.child(app_badge(SharedString::from(format!("account-agent-{id}")), "AGENT", p))
            })
            .when(kind(a) == pb::AccountKind::Linked, |el| {
                el.child(icon("link-2").size(px(14.0)).text_color(p.muted_foreground))
            })
            .when(kind(a) == pb::AccountKind::Sso, |el| {
                el.child(icon("building").size(px(14.0)).text_color(p.muted_foreground))
            });

        // The "…" menu: what an admin can do to someone else, and copying the id in developer mode.
        let menu_button = (!me || developer).then(|| {
            let (hover_bg, hover_fg) = (p.muted, p.foreground);
            let row_id = id.clone();
            div()
                .id(SharedString::from(format!("account-menu-{id}")))
                .size(px(36.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius_xl())
                .text_color(if open { p.foreground } else { p.muted_foreground })
                .when(open, |el| el.bg(p.muted))
                .when(!busy, |el| {
                    el.cursor_pointer().hover(move |s| s.bg(hover_bg).text_color(hover_fg)).on_click(cx.listener(
                        move |this, e: &gpui_kit::ClickEvent, _, cx| {
                            cx.stop_propagation();
                            let at = e.position();
                            this.accounts.menu = match &this.accounts.menu {
                                Some((m, _)) if *m == row_id => None,
                                _ => Some((row_id.clone(), at)),
                            };
                            cx.notify();
                        },
                    ))
                })
                .child(if busy {
                    spinner(SharedString::from(format!("account-busy-{id}")), 16.0, window)
                } else {
                    icon("ellipsis").size(px(16.0)).into_any_element()
                })
        });

        let off_line = a.disabled.then(|| {
            let day = a
                .disabled_at
                .as_ref()
                .map(|t| manage::day_label(t.seconds * 1000, now).to_lowercase())
                .unwrap_or_default();
            if a.disabled_reason.is_empty() {
                gpui_kit::StyledText::new(t_with("instancesettings.accounts.offNoReason", &[("date", Arg::Str(&day))]))
            } else {
                let template = t_with(
                    "instancesettings.accounts.offReason",
                    &[("date", Arg::Str(&day)), ("reason", Arg::Str("{reason}"))],
                );
                let (before, after) = template.split_once("{reason}").unwrap_or((template.as_str(), ""));
                let text = format!("{before}{}{after}", a.disabled_reason);
                let italic =
                    gpui_kit::HighlightStyle { font_style: Some(gpui_kit::FontStyle::Italic), ..Default::default() };
                gpui_kit::StyledText::new(text)
                    .with_highlights([(before.len()..before.len() + a.disabled_reason.len(), italic)])
            }
        });
        // `rounded-2xl border bg-background/40 p-2.5 pr-2`, red-tinged while turned off.
        let (tint, edge, hover_edge, hover_bg): (Hsla, Hsla, Hsla, Hsla) = if a.disabled {
            (alpha(destructive, 0.05), alpha(destructive, 0.3), alpha(destructive, 0.4), alpha(destructive, 0.1))
        } else {
            (alpha(p.background, 0.4), p.border.into(), alpha(p.primary, 0.3), alpha(p.muted, 0.4))
        };
        let row = div()
            .id(SharedString::from(format!("account-{id}")))
            .h(px(ROW))
            .flex()
            .flex_col()
            .justify_center()
            .p(px(10.0))
            .pr(px(8.0))
            .rounded(radius_2xl())
            .bg(tint)
            .border_1()
            .border_color(edge)
            .hover(move |s| s.border_color(hover_edge).bg(hover_bg))
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
                            .child(badges)
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(format!("{handle} · {when}")),
                            )
                            // Rows are all one height, so why it's off takes the facts' line.
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .map(|el| match off_line {
                                        Some(line) => el.text_color(alpha(destructive, 0.9)).child(line),
                                        None => el
                                            .text_color(alpha(p.muted_foreground, 0.8))
                                            .child(gpui_kit::StyledText::new(facts.join(" · "))),
                                    }),
                            ),
                    )
                    .when_some(menu_button, |el, b| el.child(b)),
            );
        motion::rise(
            row,
            SharedString::from(format!("account-in-{id}")),
            Duration::from_millis(20 * index.min(14) as u64),
            10.0,
        )
        .into_any_element()
    }

    /// Closes a row's "…" menu, if one's open.
    pub(super) fn close_account_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if self.accounts.menu.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    /// A row's "…" menu (the web's `DropdownMenu`, `w-56`, aligned to the button's end).
    pub(super) fn account_menu(&mut self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, at) = self.accounts.menu.clone()?;
        let a = self.accounts.list.as_ref()?.iter().find(|x| id_of(x) == id)?.clone();
        let me = self.core.shared.read(|s| s.instance(&self.key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())))
            == Some(id.clone());
        let developer = self.core.prefs().developer_mode;
        let name = name_of(&a);
        let agent = is_agent(a.user.as_ref());
        let item = |key: &'static str,
                    glyph: &'static str,
                    label: String,
                    danger: bool,
                    run: Action,
                    cx: &mut Context<Self>| {
            let (hover, fg, glyph_fg): (Hsla, Hsla, Hsla) = if danger {
                (alpha(p.destructive, if p.dark { 0.2 } else { 0.1 }), p.destructive.into(), p.destructive.into())
            } else {
                (p.accent.into(), p.foreground.into(), p.muted_foreground.into())
            };
            div()
                .id(SharedString::from(format!("account-menu-{key}")))
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
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.accounts.menu = None;
                    run(this, window, cx);
                    cx.notify();
                }))
                .child(icon(glyph).size(px(16.0)).text_color(glyph_fg))
                .child(label)
        };
        let separator = || div().mx(px(-4.0)).my(px(4.0)).h(px(1.0)).bg(p.border);
        let mut items = div().flex().flex_col();
        if !me {
            if !a.admin && !a.disabled && !agent {
                let (acc, n) = (a.clone(), name.clone());
                items = items.child(item(
                    "admin",
                    "shield",
                    t("instancesettings.accounts.makeAdmin"),
                    false,
                    Box::new(move |this, window, cx| {
                        let done = t_with("instancesettings.accounts.madeAdmin", &[("name", Arg::Str(&n))]);
                        this.change_account(&acc, Some(true), None, done, window, cx)
                    }),
                    cx,
                ));
            }
            if a.admin {
                let (acc, n) = (a.clone(), name.clone());
                items = items.child(item(
                    "unadmin",
                    "shield-off",
                    t("instancesettings.accounts.removeAdmin"),
                    false,
                    Box::new(move |this, window, cx| {
                        let done = t_with("instancesettings.accounts.unmadeAdmin", &[("name", Arg::Str(&n))]);
                        this.change_account(&acc, Some(false), None, done, window, cx)
                    }),
                    cx,
                ));
            }
            if kind(&a) == pb::AccountKind::Local {
                let acc = a.clone();
                items = items.child(item(
                    "reset",
                    "key-round",
                    t("instancesettings.accounts.resetPassword"),
                    false,
                    Box::new(move |this, window, cx| this.ask(Pending::Reset(acc.clone()), window, cx)),
                    cx,
                ));
            }
            items = items.child(separator());
            if a.disabled {
                let (acc, n) = (a.clone(), name.clone());
                items = items.child(item(
                    "on",
                    "power",
                    t("instancesettings.accounts.turnBackOn"),
                    false,
                    Box::new(move |this, window, cx| {
                        let done = t_with("instancesettings.accounts.turnedOn", &[("name", Arg::Str(&n))]);
                        this.change_account(&acc, None, Some(false), done, window, cx)
                    }),
                    cx,
                ));
            } else {
                let acc = a.clone();
                items = items.child(item(
                    "off",
                    "power-off",
                    t("accountsettings.shared.turnOff"),
                    true,
                    Box::new(move |this, window, cx| this.ask(Pending::TurnOff(acc.clone()), window, cx)),
                    cx,
                ));
            }
        }
        if developer {
            if !me {
                items = items.child(separator());
            }
            let copied = id.clone();
            let what = t("common.copy.accountId");
            let done = t_with("common.copied", &[("what", Arg::Str(&what))]);
            items = items.child(item(
                "copy-id",
                "fingerprint-pattern",
                t_with("common.copyThing", &[("what", Arg::Str(&what))]),
                false,
                Box::new(move |this, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                    this.toast("copy", done.clone(), cx);
                }),
                cx,
            ));
        }
        let x = f32::from(at.x) - 224.0 + 18.0;
        let y = f32::from(at.y) + 22.0;
        let panel = div()
            .id("account-menu-panel")
            .absolute()
            .left(px(x.max(8.0)))
            .top(px(y))
            .w(px(224.0))
            .p(px(4.0))
            .rounded(radius_md())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .text_color(p.foreground)
            .shadow(shadow_md())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(items);
        Some(
            div()
                .id("account-menu-layer")
                .absolute()
                .inset_0()
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_account_menu(cx);
                }))
                .child(motion::once(
                    panel,
                    SharedString::from(format!("account-menu-in-{id}")),
                    Duration::from_millis(160),
                    |el, t| {
                        let k = 1.0 - (1.0 - t).powi(3);
                        el.opacity(k).mt(px(-6.0 * (1.0 - k)))
                    },
                ))
                .into_any_element(),
        )
    }

    /// The dialog over the list: turning someone off, or a new password (the web's `TurnOff` and
    /// `ResetPassword`).
    pub(super) fn account_dialog(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pending = self.accounts.pending.clone()?;
        let a = &self.accounts;
        let busy = a.dialog_busy;
        let account = match &pending {
            Pending::TurnOff(account) | Pending::Reset(account) => account.clone(),
        };
        let name = name_of(&account);
        // `flex items-center gap-3 rounded-2xl bg-muted/60 p-3`.
        let who = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(12.0))
            .rounded(radius_2xl())
            .bg(alpha(p.muted, 0.6))
            .child(avatar(account.user.as_ref(), 40.0, p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div().font_weight(FontWeight::BOLD).line_height(px(24.0)).text_ellipsis().child(name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", account.user.as_ref().map(|u| u.username.as_str()).unwrap_or(""))),
                    ),
            );
        let error = a.dialog_error.clone().map(|e| {
            div().text_sm().line_height(px(20.0)).text_color(p.destructive).child(capitalized(&e)).into_any_element()
        });
        let cancel = button("account-dialog-cancel", t("common.cancel"), None, Look::Ghost, false, p)
            .rounded(radius_xl())
            .when(busy, |el| el.opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| {
                if !this.accounts.dialog_busy {
                    this.close_account_dialog(cx);
                }
            }));
        let (title, description, body): (String, String, AnyElement) = match (&pending, &a.password) {
            (Pending::Reset(_), Some(password)) => {
                let two = a.two_factor;
                (
                    t_with("instancesettings.accounts.newPasswordTitle", &[("name", Arg::Str(&name))]),
                    t("instancesettings.accounts.newPasswordHint"),
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .child(self.new_password(password, p, window, cx))
                        .when(two, |el| {
                            el.child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(t("instancesettings.accounts.twoStepOff")),
                            )
                        })
                        .child(
                            div().flex().justify_end().child(
                                button(
                                    "account-dialog-done",
                                    t("accountsettings.shared.done"),
                                    None,
                                    Look::Primary,
                                    false,
                                    p,
                                )
                                .rounded(radius_xl())
                                .px(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_account_dialog(cx);
                                })),
                            ),
                        )
                        .into_any_element(),
                )
            }
            (Pending::Reset(_), None) => {
                let two = a.two_factor;
                let content = div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(who)
                    .when(account.two_factor, |el| {
                        let hover = alpha(p.primary, 0.3);
                        el.child(
                            div()
                                .id("reset-two-factor")
                                .flex()
                                .items_start()
                                .gap(px(12.0))
                                .p(px(12.0))
                                .rounded(radius_xl())
                                .border_1()
                                .border_color(p.border)
                                .cursor_pointer()
                                .hover(move |s| s.border_color(hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.accounts.two_factor = !two;
                                    cx.notify();
                                }))
                                .child(div().mt(px(2.0)).child(switch(
                                    "reset-two-factor-switch",
                                    two,
                                    false,
                                    p,
                                    window,
                                    cx,
                                    |this, on, _, cx| {
                                        this.accounts.two_factor = on;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .child(t("instancesettings.accounts.alsoTwoStep")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.muted_foreground)
                                                .child(t("instancesettings.accounts.alsoTwoStepHint")),
                                        ),
                                ),
                        )
                    })
                    .when_some(error, |el, e| el.child(e))
                    .child(
                        dialog_buttons().child(cancel).child(
                            button(
                                "account-dialog-ok",
                                t("instancesettings.accounts.resetPassword"),
                                if busy { None } else { Some("key-round") },
                                Look::Primary,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .when(busy, |el| el.opacity(0.5).child(spinner("account-dialog-spin", 16.0, window)))
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                        ),
                    );
                (
                    t_with("instancesettings.accounts.resetTitle", &[("name", Arg::Str(&name))]),
                    t("instancesettings.accounts.resetHint"),
                    content.into_any_element(),
                )
            }
            (Pending::TurnOff(_), _) => {
                let content = div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(who)
                    .when(account.admin, |el| {
                        el.child(
                            div()
                                .px(px(12.0))
                                .py(px(8.0))
                                .rounded(radius_xl())
                                .bg(alpha(gpui_kit::rgb(0xfe9a00), 0.1))
                                .text_sm()
                                .text_color(amber_text(p))
                                .child(t("instancesettings.accounts.turnOffAdmin")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .child(t("instancesettings.accounts.reason")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.muted_foreground)
                                            .child(t("instancesettings.accounts.reasonHint")),
                                    ),
                            )
                            .child(focus_ring(
                                area_box(Textarea::new(&a.reason).appearance(false), None, p),
                                has_focus(&a.reason, window, cx),
                                p,
                            )),
                    )
                    .when_some(error, |el, e| el.child(e))
                    .child(
                        dialog_buttons().child(cancel).child(
                            button(
                                "account-dialog-ok",
                                t("accountsettings.shared.turnOff"),
                                if busy { None } else { Some("power-off") },
                                Look::Destructive,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .when(busy, |el| el.opacity(0.5).child(spinner("account-dialog-spin", 16.0, window)))
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                        ),
                    );
                (
                    t_with("instancesettings.accounts.turnOffTitle", &[("name", Arg::Str(&name))]),
                    t("instancesettings.accounts.turnOffHint"),
                    content.into_any_element(),
                )
            }
        };
        let tag = match (&pending, a.password.is_some()) {
            (Pending::TurnOff(_), _) => "off",
            (Pending::Reset(_), false) => "reset",
            (Pending::Reset(_), true) => "password",
        };
        // The web's header sits inside the form's `gap-4` column, so the body starts that much lower.
        Some(dialog(
            &format!("account-dialog-{tag}-{}", id_of(&account)),
            title,
            Some(description),
            div().pt(px(16.0)).child(body),
            p,
            cx,
            |this, cx| {
                if !this.accounts.dialog_busy {
                    this.close_account_dialog(cx);
                }
            },
        ))
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
            .rounded(radius_xl())
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
                        .rounded(radius_xl())
                        .bg(alpha(p.background, 0.6))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.accounts.revealed = true;
                            cx.notify();
                        }))
                        .child(icon("eye").size(px(16.0)))
                        .child(t("instancesettings.accounts.hiddenShow")),
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
            .rounded(radius_xl())
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
                    gpui_kit::rgb(0x00bc7d)
                } else {
                    p.foreground
                })),
                SharedString::from(format!("password-copied-{copied}")),
                Duration::from_millis(300),
                |el, t| el.opacity(t),
            ));
        div().flex().items_center().gap(px(8.0)).child(code).child(copy).into_any_element()
    }
}

/// An instance's message with its first letter up, as the web shows them.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
