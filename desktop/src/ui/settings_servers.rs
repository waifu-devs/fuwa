//! The account pages about your servers on the instance, as the web's
//! `ServerProfiles.tsx` (a nickname in each server, beside the card people
//! there open) and `ServerNotifications.tsx` (how each server and channel
//! notifies you, kept on the instance for every device).

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::account::NotificationPatch;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::notifications::{is_muted, key as nkey};
use crate::core::store::user_name;
use crate::pb::{self, NotificationLevel as Level};
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{At, Look, button, field, hint, save_bar, segmented, toggle};
use crate::ui::settings_menu::Item;
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{icon, server_icon};

/// What the two pages keep.
pub(crate) struct ServersForm {
    pub server: Option<String>,
    nickname: Option<Entity<InputState>>,
    /// The nickname the field was filled with, for the server it's for.
    filled: Option<(String, String)>,
    saving: bool,
    error: Option<String>,
    /// The server whose notifications are open.
    pub open: Option<String>,
    /// Channels set apart on the page even with no settings yet.
    added: std::collections::HashSet<String>,
}

impl Default for ServersForm {
    fn default() -> Self {
        Self {
            server: None,
            nickname: None,
            filled: None,
            saving: false,
            error: None,
            open: None,
            added: Default::default(),
        }
    }
}

const MUTE_FOR: [Option<i64>; 6] =
    [Some(15 * 60_000), Some(60 * 60_000), Some(3 * 60 * 60_000), Some(8 * 60 * 60_000), Some(24 * 60 * 60_000), None];

fn mute_label(ms: Option<i64>) -> String {
    match ms {
        None => t("common.notify.forever"),
        Some(ms) if ms < 60 * 60_000 => t_with("common.notify.forMinutes", &[("count", Arg::Num(ms / 60_000))]),
        Some(ms) => t_with("common.notify.forHours", &[("count", Arg::Num(ms / 3_600_000))]),
    }
}

fn muted_label(n: Option<&pb::NotificationSettings>) -> String {
    use chrono::TimeZone as _;
    let until = n
        .and_then(|n| n.muted_until.as_ref())
        .and_then(|u| chrono::Local.timestamp_millis_opt(u.seconds * 1000).single());
    match until {
        None => t("common.notify.muted"),
        Some(at) => {
            let ms = at.timestamp_millis();
            let time = crate::ui::text::clock(ms);
            let time = if at.date_naive() == chrono::Local::now().date_naive() {
                time
            } else {
                format!("{} {time}", at.format("%a"))
            };
            t_with("common.notify.mutedUntil", &[("time", Arg::Str(&time))])
        }
    }
}

const LEVELS: [(Level, &str, &str); 3] = [
    (Level::All, "common.notify.all", "common.notify.allShort"),
    (Level::Mentions, "common.notify.mentions", "common.notify.mentionsShort"),
    (Level::Nothing, "common.notify.nothing", "common.notify.nothingShort"),
];

fn level_label(level: Level, server_default: i32) -> String {
    if let Some((_, long, _)) = LEVELS.iter().find(|(l, ..)| *l == level) {
        return t(long);
    }
    match LEVELS.iter().find(|(l, ..)| *l as i32 == server_default) {
        Some((_, long, _)) => {
            t_with("accountsettings.serverNotifications.defaultIs", &[("level", Arg::Str(&t(long).to_lowercase()))])
        }
        None => t("accountsettings.serverNotifications.default"),
    }
}

/// No servers yet: nothing to pick.
fn no_servers(hint_key: &str, p: &Palette) -> AnyElement {
    motion::rise(
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_3xl())
            .border_1()
            .border_dashed()
            .border_color(p.border)
            .px(px(24.0))
            .py(px(48.0))
            .text_center()
            .child(icon("server").size(px(32.0)).text_color(p.muted_foreground))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("accountsettings.shared.noServers")))
            .child(div().max_w(px(384.0)).text_sm().text_color(p.muted_foreground).child(t(hint_key))),
        "no-servers",
        Duration::ZERO,
        8.0,
    )
    .into_any_element()
}

impl SettingsView {
    pub(crate) fn server_profiles_page(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((key, me)) = self.account_ready(window, cx) else { return div().into_any_element() };
        let servers = self.core.shared.read(|s| s.instance(&key).map(|i| i.servers.clone()).unwrap_or_default());
        if servers.is_empty() {
            return no_servers("accountsettings.serverProfiles.noServersHint", p);
        }
        let server_id = self
            .servers
            .server
            .clone()
            .filter(|id| servers.iter().any(|s| &s.id == id))
            .unwrap_or_else(|| servers[0].id.clone());
        self.servers.server = Some(server_id.clone());
        let member = self.core.shared.read(|s| {
            s.instance(&key)
                .and_then(|i| i.members.get(&server_id))
                .and_then(|list| list.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == me.id)).cloned())
        });
        let saved = member.as_ref().map(|m| m.nickname.clone()).unwrap_or_default();
        let state = match &self.servers.nickname {
            Some(s) => s.clone(),
            None => {
                let state = cx.new(|cx| InputState::new(window, cx));
                cx.subscribe(&state, |_, _, _: &InputEvent, cx| cx.notify()).detach();
                self.servers.nickname = Some(state.clone());
                state
            }
        };
        if self.servers.filled.as_ref().map(|(s, _)| s) != Some(&server_id) {
            self.servers.filled = Some((server_id.clone(), saved.clone()));
            let (saved, name) = (saved.clone(), user_name(&me));
            state.update(cx, |s, cx| {
                s.set_value(saved, window, cx);
                s.set_placeholder(name, window, cx);
            });
        }
        let value = state.read(cx).value().to_string();
        let changed = usize::from(value.trim() != saved);
        if changed > 0 {
            self.holding = true;
        }
        let server_name = servers
            .iter()
            .find(|s| s.id == server_id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| t("accountsettings.serverProfiles.thisServer"));

        let mut picks = div().flex().flex_wrap().gap(px(8.0));
        for s in &servers {
            let active = s.id == server_id;
            let locked = changed > 0 && !active;
            let id = s.id.clone();
            let (hover_border, hover_fg) = (alpha(p.primary, 0.3), p.foreground);
            picks = picks.child(
                div()
                    .id(SharedString::from(format!("sp-{}", s.id)))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .py(px(6.0))
                    .pl(px(6.0))
                    .pr(px(12.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .map(|el| {
                        if active {
                            el.border_color(alpha(p.primary, 0.6)).bg(alpha(p.primary, 0.1))
                        } else {
                            el.border_color(p.border).text_color(p.muted_foreground)
                        }
                    })
                    .when(locked, |el| el.opacity(0.5))
                    .when(!locked && !active, |el| {
                        el.cursor_pointer()
                            .hover(move |s| s.border_color(hover_border).text_color(hover_fg).top(px(-2.0)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.servers.server = Some(id.clone());
                                this.servers.error = None;
                                cx.notify();
                            }))
                    })
                    .child(server_icon(s, 28.0, 8.0, p))
                    .child(div().truncate().child(s.name.clone())),
            );
        }
        let nick_hint = div()
            .text_sm()
            .text_color(p.muted_foreground)
            .child(crate::ui::text::hint_line(
                &t("accountsettings.serverProfiles.nicknameHint"),
                &[("server", &server_name), ("name", &user_name(&me))],
                p,
            ))
            .into_any_element();
        let nick = motion::slide_in(
            div()
                .max_w(px(384.0))
                .when(member.is_none(), |el| el.opacity(0.5))
                .child(field(Input::new(&state).appearance(false), p)),
            SharedString::from(format!("nick-{server_id}")),
            12.0,
        );
        let form = div()
            .flex()
            .flex_col()
            .child(self.row("server", &t("accountsettings.serverProfiles.server"), None, At::of(0, 2), picks, p))
            .child(self.row("nickname", &t("settings.nav.nickname"), Some(nick_hint), At::of(1, 2), nick, p));
        let base = self.saved_draft();
        let preview = self.profile_preview(&me, &base, Some(value.trim()), p, cx);
        let alarm = self.alarm();
        let (k, sid) = (key.clone(), server_id.clone());
        let bar = save_bar(
            "nickname",
            changed,
            self.servers.saving,
            self.servers.error.as_deref(),
            alarm,
            p,
            cx,
            move |this, _, cx| {
                let value =
                    this.servers.nickname.as_ref().map(|s| s.read(cx).value().trim().to_owned()).unwrap_or_default();
                this.servers.saving = true;
                let (core, k, sid) = (this.core.clone(), k.clone(), sid.clone());
                let rx = this.core.spawn(async move { core.set_nickname(&k, &sid, &value).await });
                cx.spawn(async move |this, cx| {
                    let Ok(result) = rx.await else { return };
                    let _ = this.update(cx, |this, cx| {
                        this.servers.saving = false;
                        match result {
                            Ok(_) => this.servers.filled = None,
                            Err(e) => this.servers.error = Some(e.message),
                        }
                        cx.notify();
                    });
                })
                .detach();
            },
            |this, _, cx| {
                this.servers.filled = None;
                this.servers.error = None;
                cx.notify();
            },
        );
        div()
            .flex()
            .flex_col()
            .child(crate::ui::settings_controls::with_preview(form, preview, self.wide, p))
            .child(bar)
            .into_any_element()
    }

    fn change_notifications(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        patch: NotificationPatch,
        cx: &mut Context<Self>,
    ) {
        let (core, k, s, c) = (self.core.clone(), key.to_owned(), server.to_owned(), channel.to_owned());
        let rx = self.core.spawn(async move { core.update_notifications(&k, &s, &c, patch).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.toast("circle-alert", e.message, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn server_notifications_page(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        let servers = self.core.shared.read(|s| s.instance(&key).map(|i| i.servers.clone()).unwrap_or_default());
        if servers.is_empty() {
            return no_servers("accountsettings.serverNotifications.noServersHint", p);
        }
        let now = crate::core::dms::now_ms();
        let mut list =
            div().flex().flex_col().gap(px(12.0)).child(hint(t("accountsettings.serverNotifications.intro"), p));
        for (n, server) in servers.iter().enumerate() {
            let settings = self
                .core
                .shared
                .read(|s| s.instance(&key).and_then(|i| i.notifications.get(&nkey(&server.id, "")).cloned()));
            let muted = is_muted(settings.as_ref(), now);
            let open = self.servers.open.as_deref() == Some(server.id.as_str());
            let sid = server.id.clone();
            let level = settings.as_ref().map(|s| s.level()).unwrap_or(Level::Unspecified);
            let line = if muted {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xf59e0b))
                    .child(icon("bell-off").size(px(14.0)))
                    .child(muted_label(settings.as_ref()))
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon("bell").size(px(14.0)))
                    .child(level_label(level, server.default_notifications))
            };
            let hover = alpha(p.muted, 0.5);
            let head = div()
                .id(SharedString::from(format!("sn-{}", server.id)))
                .flex()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.servers.open =
                        if this.servers.open.as_deref() == Some(sid.as_str()) { None } else { Some(sid.clone()) };
                    cx.notify();
                }))
                .child(server_icon(server, 40.0, 12.0, p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::BOLD).truncate().child(server.name.clone()))
                        .child(div().text_xs().text_color(p.muted_foreground).child(line)),
                )
                .child(
                    icon(if open { "chevron-up" } else { "chevron-down" })
                        .size(px(16.0))
                        .text_color(p.muted_foreground),
                );
            let mut card = div()
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_1()
                .border_color(if open { alpha(p.primary, 0.4) } else { p.border.into() })
                .bg(p.card)
                .child(head);
            if open {
                card = card.child(motion::rise(
                    self.server_body(&key, server, settings.as_ref(), muted, now, p, window, cx),
                    SharedString::from(format!("sn-body-{}", server.id)),
                    Duration::ZERO,
                    -8.0,
                ));
            }
            list = list.child(motion::rise(
                div().child(card),
                SharedString::from(format!("sn-in-{n}")),
                Duration::from_millis(40 * n as u64),
                10.0,
            ));
        }
        list.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn server_body(
        &mut self,
        key: &str,
        server: &pb::Server,
        settings: Option<&pb::NotificationSettings>,
        muted: bool,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let level = settings.map(|s| s.level()).unwrap_or(Level::Unspecified);
        let options: Vec<(String, Option<&'static str>)> =
            std::iter::once(t("accountsettings.serverNotifications.default"))
                .chain(LEVELS.iter().map(|(_, _, short)| t(short)))
                .map(|l| (l, None))
                .collect();
        let at = match level {
            Level::All => 1,
            Level::Mentions => 2,
            Level::Nothing => 3,
            _ => 0,
        };
        let (k, sid) = (key.to_owned(), server.id.clone());
        let levels = segmented(
            leak_id(&format!("sn-level-{}", server.id)),
            options,
            at,
            ((self.column.min(448.0)) - 8.0) / 4.0,
            p,
            window,
            cx,
            move |this, n, cx| {
                let level = [Level::Unspecified, Level::All, Level::Mentions, Level::Nothing][n];
                this.change_notifications(
                    &k,
                    &sid,
                    "",
                    NotificationPatch { level: Some(level), ..Default::default() },
                    cx,
                )
            },
        );
        let (k, sid) = (key.to_owned(), server.id.clone());
        let suppress = toggle(
            leak_id(&format!("sn-suppress-{}", server.id)),
            &t("accountsettings.serverNotifications.suppress"),
            Some(&t("accountsettings.serverNotifications.suppressHint")),
            settings.is_some_and(|s| s.suppress_everyone),
            false,
            p,
            window,
            cx,
            move |this, on, cx| {
                this.change_notifications(
                    &k,
                    &sid,
                    "",
                    NotificationPatch { suppress_everyone: Some(on), ..Default::default() },
                    cx,
                )
            },
        );
        // Channels with their own settings, and the rest to add.
        let (channels, all) = self.core.shared.read(|s| {
            let i = s.instance(key);
            (
                i.and_then(|i| i.channels.get(&server.id).cloned()).unwrap_or_default(),
                i.map(|i| i.notifications.clone()).unwrap_or_default(),
            )
        });
        let openable: Vec<pb::Channel> =
            channels.into_iter().filter(|c| c.r#type != pb::ChannelType::Category as i32).collect();
        let apart: Vec<pb::Channel> = openable
            .iter()
            .filter(|c| all.contains_key(&nkey(&server.id, &c.id)) || self.servers.added.contains(&c.id))
            .cloned()
            .collect();
        let rest: Vec<pb::Channel> = openable.iter().filter(|c| !apart.iter().any(|a| a.id == c.id)).cloned().collect();
        let mut items = vec![Item::Label(t("accountsettings.serverNotifications.setApart"))];
        for c in &rest {
            let id = c.id.clone();
            items.push(Item::action(c.name.clone(), Some(channel_glyph(c)), move |this, cx| {
                this.servers.added.insert(id.clone());
                cx.notify();
            }));
        }
        let add = self.dropdown(
            format!("sn-add-{}", server.id),
            button(
                SharedString::from(format!("sn-add-btn-{}", server.id)),
                t("accountsettings.serverNotifications.addChannel"),
                Some("plus"),
                Look::Outline,
                true,
                p,
            )
            .rounded(radius_xl())
            .when(rest.is_empty(), |el| el.opacity(0.5)),
            items,
            true,
            224.0,
            p,
            cx,
        );
        let mut rows = div().flex().flex_col().gap(px(8.0));
        if apart.is_empty() {
            rows = rows.child(
                div()
                    .rounded(radius_xl())
                    .border_1()
                    .border_dashed()
                    .border_color(p.border)
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(t("accountsettings.serverNotifications.noChannels")),
            );
        }
        for c in &apart {
            rows = rows.child(self.channel_row(key, &server.id, c, now, p, window, cx));
        }
        let (k, sid) = (key.to_owned(), server.id.clone());
        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .border_t_1()
            .border_color(p.border)
            .p(px(16.0))
            .child(self.mute_control(
                &format!("sn-mute-{}", server.id),
                muted,
                &muted_label(settings),
                false,
                true,
                p,
                cx,
                move |this, until, cx| this.change_notifications(&k, &sid, "", until, cx),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div().text_sm().font_weight(FontWeight::BOLD).child(t("appsettings.notifications.notifyFor")),
                    )
                    .child(div().flex().child(levels))
                    .when(
                        level == Level::Unspecified && server.default_notifications == Level::Mentions as i32,
                        |el| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .child(t("accountsettings.serverNotifications.mentionsDefault")),
                            )
                        },
                    ),
            )
            .child(suppress)
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
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(t("accountsettings.serverNotifications.channels")),
                            )
                            .child(add),
                    )
                    .child(rows),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn channel_row(
        &mut self,
        key: &str,
        server: &str,
        c: &pb::Channel,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let settings =
            self.core.shared.read(|s| s.instance(key).and_then(|i| i.notifications.get(&nkey(server, &c.id)).cloned()));
        let muted = is_muted(settings.as_ref(), now);
        let level = settings.as_ref().map(|s| s.level()).unwrap_or(Level::Unspecified);
        let at = match level {
            Level::All => 1,
            Level::Mentions => 2,
            Level::Nothing => 3,
            _ => 0,
        };
        let options: Vec<(String, Option<&'static str>)> =
            std::iter::once(t("accountsettings.serverNotifications.default"))
                .chain(LEVELS.iter().map(|(_, _, short)| t(short)))
                .map(|l| (l, None))
                .collect();
        let (k, s, ch) = (key.to_owned(), server.to_owned(), c.id.clone());
        let levels =
            segmented(leak_id(&format!("cn-level-{}", c.id)), options, at, 80.0, p, window, cx, move |this, n, cx| {
                let level = [Level::Unspecified, Level::All, Level::Mentions, Level::Nothing][n];
                this.servers.added.insert(ch.clone());
                this.change_notifications(
                    &k,
                    &s,
                    &ch,
                    NotificationPatch { level: Some(level), ..Default::default() },
                    cx,
                )
            });
        let (k, s, ch) = (key.to_owned(), server.to_owned(), c.id.clone());
        let mute = self.mute_control(
            &format!("cn-mute-{}", c.id),
            muted,
            &muted_label(settings.as_ref()),
            true,
            false,
            p,
            cx,
            move |this, until, cx| {
                this.servers.added.insert(ch.clone());
                this.change_notifications(&k, &s, &ch, until, cx)
            },
        );
        let (k, s, ch) = (key.to_owned(), server.to_owned(), c.id.clone());
        let had = settings.is_some();
        let (hover_bg, hover_fg) = (p.muted, p.foreground);
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.5))
                .p(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(icon(channel_glyph(c)).size(px(16.0)).text_color(p.muted_foreground))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(c.name.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("cn-rm-{}", c.id)))
                                .size(px(28.0))
                                .rounded(radius_lg())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted_foreground)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.servers.added.remove(&ch);
                                    if had {
                                        this.change_notifications(
                                            &k,
                                            &s,
                                            &ch,
                                            NotificationPatch {
                                                level: Some(Level::Unspecified),
                                                unmute: true,
                                                ..Default::default()
                                            },
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                }))
                                .child(icon("x").size(px(16.0))),
                        ),
                )
                .child(div().flex().flex_wrap().items_center().gap(px(8.0)).child(levels).child(mute)),
            SharedString::from(format!("cn-in-{}", c.id)),
            Duration::ZERO,
            -8.0,
        )
        .into_any_element()
    }

    /// Mute for a while (picked from a menu), or unmute.
    #[allow(clippy::too_many_arguments)]
    fn mute_control(
        &self,
        id: &str,
        muted: bool,
        label: &str,
        compact: bool,
        server: bool,
        p: &Palette,
        cx: &mut Context<Self>,
        change: impl Fn(&mut Self, NotificationPatch, &mut Context<Self>) + Clone + 'static,
    ) -> AnyElement {
        let control: AnyElement = if muted {
            let change = change.clone();
            let hover = alpha(rgb(0xf59e0b), 0.1);
            div()
                .id(SharedString::from(format!("{id}-unmute")))
                .flex_none()
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(radius_xl())
                .border_1()
                .border_color(alpha(rgb(0xf59e0b), 0.4))
                .bg(p.background)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xd97706))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    change(this, NotificationPatch { unmute: true, ..Default::default() }, cx)
                }))
                .child(icon("bell-off").size(px(16.0)))
                .child(label.to_owned())
                .into_any_element()
        } else {
            let items = MUTE_FOR
                .into_iter()
                .map(|ms| {
                    let change = change.clone();
                    Item::action(mute_label(ms), None, move |this, cx| {
                        let until = ms.map(|ms| crate::core::dms::now_ms() + ms);
                        change(this, NotificationPatch { mute_until: Some(until), ..Default::default() }, cx)
                    })
                })
                .collect();
            self.dropdown(
                id.to_owned(),
                button(
                    SharedString::from(format!("{id}-btn")),
                    t("accountsettings.serverNotifications.mute"),
                    Some("bell-off"),
                    Look::Outline,
                    true,
                    p,
                )
                .rounded(radius_xl()),
                items,
                true,
                208.0,
                p,
                cx,
            )
        };
        if compact {
            return control;
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .min_w_0()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(if server {
                        t("accountsettings.serverNotifications.muteServer")
                    } else {
                        t("accountsettings.serverNotifications.muteChannel")
                    }))
                    .child(div().text_xs().text_color(p.muted_foreground).child(if server {
                        t("accountsettings.serverNotifications.muteServerHint")
                    } else {
                        t("accountsettings.serverNotifications.muteChannelHint")
                    })),
            )
            .child(control)
            .into_any_element()
    }
}

fn channel_glyph(c: &pb::Channel) -> &'static str {
    match pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text) {
        pb::ChannelType::Voice => "volume-2",
        pb::ChannelType::Announcement => "megaphone",
        _ => "hash",
    }
}

/// An id for an element that has to live as long as the app, made once per name.
pub(crate) fn leak_id(name: &str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static IDS: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut ids = IDS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    ids.entry(name.to_owned()).or_insert_with(|| Box::leak(name.to_owned().into_boxed_str()))
}
