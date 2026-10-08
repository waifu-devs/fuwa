//! The people pages, as the web's `settings/server/Invites.tsx`,
//! `Members.tsx` and `Bans.tsx`: the invite links still working (who made
//! each, where it leads, its uses and time left), everyone in the server
//! highest ranked first with their roles and a menu of what you may do to
//! them, and who's banned and why.

use std::collections::HashSet;

use gpui_kit::{App, Div, Stateful};

use super::menu::{Item, lead_icon, role_dot};
use super::pages::{boxed, focused};
use super::*;
use crate::core::moderation::outranks;
use crate::core::permissions::Access;
use crate::ui::settings_controls::{Look, button};
use crate::ui::theme::{radius_2xl, radius_3xl, radius_lg, radius_xl};

pub(super) struct PeopleState {
    /// Members: only those with this role, and only those timed out.
    role: Option<String>,
    timed_out: bool,
    bans_query: Entity<InputState>,
    /// The ban being lifted.
    lifting: Option<String>,
    /// Roles being given or taken, by member and role.
    toggling: HashSet<(String, String)>,
    /// Changing someone's nickname: who, the box, and how it went.
    pub(super) nickname: Option<String>,
    nick: Entity<InputState>,
    nick_busy: bool,
    nick_error: Option<String>,
    /// The audit log: only what this person did (empty for anyone).
    pub(super) audit_actor: String,
}

impl PeopleState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let bans_query = cx.new(|cx| InputState::new(window, cx).placeholder(t("serversettings.bans.search")));
        let nick = cx.new(|cx| InputState::new(window, cx));
        let notify = |cx: &mut Context<ServerSettingsView>, e: &Entity<InputState>| {
            cx.subscribe(e, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    cx.notify()
                }
            })
        };
        let subs = vec![notify(cx, &bans_query), notify(cx, &nick)];
        (
            Self {
                role: None,
                timed_out: false,
                bans_query,
                lifting: None,
                toggling: HashSet::new(),
                nickname: None,
                nick,
                nick_busy: false,
                nick_error: None,
                audit_actor: String::new(),
            },
            subs,
        )
    }
}

/// What's left of a time-out, as the web's `formatLeft`: "4:05", "3h 12m", "4d 2h".
pub(super) fn left(ms: i64) -> String {
    let s = ((ms as f64) / 1000.0).ceil().max(0.0) as i64;
    if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3600, s % 3600 / 60)
    } else {
        format!("{}d {}h", s / 86_400, s % 86_400 / 3600)
    }
}

/// The Tailwind colors the web's pages use.
pub(super) const AMBER_400: u32 = 0xfbbf24;
pub(super) const AMBER_500: u32 = 0xf59e0b;
pub(super) const AMBER_600: u32 = 0xd97706;
pub(super) const EMERALD_500: u32 = 0x10b981;

/// The amber text of a time-out pill (`text-amber-600 dark:text-amber-400`).
pub(super) fn amber_text(p: &Palette) -> Hsla {
    gpui_kit::rgb(if p.dark { AMBER_400 } else { AMBER_600 }).into()
}

/// A search box (`h-10 rounded-xl pl-9` with the glass at `left-3`).
pub(super) fn search_box(state: &Entity<InputState>, p: &Palette, window: &Window, cx: &App) -> Div {
    div()
        .relative()
        .child(boxed(Input::new(state).appearance(false), 40.0, focused(state, window, cx), p).pl(px(28.0)))
        .child(
            div()
                .absolute()
                .left(px(12.0))
                .top(px(12.0))
                .text_color(p.muted_foreground)
                .child(icon("search").size(px(16.0))),
        )
}

/// A list's count over it (`text-xs font-bold tracking-wide uppercase` with an icon).
fn count_line(glyph: &str, text: String, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(icon(glyph).size(px(14.0)))
        .child(text.to_uppercase())
}

/// A row of a list (`rounded-2xl border bg-background/40`).
fn list_row(p: &Palette) -> Div {
    div().rounded(radius_2xl()).border_1().border_color(p.border).bg(alpha(p.background, 0.4))
}

/// The hourglass that turns while a time-out runs (`animate-[spin_3s_ease-in-out_infinite]`).
fn turning_hourglass(id: impl Into<SharedString>, window: &Window) -> AnyElement {
    motion::ambient(icon("hourglass").size(px(14.0)), id.into(), Duration::from_millis(3000), window, |el, t| {
        let e = if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 };
        el.rotate(gpui_kit::radians(e * std::f32::consts::TAU))
    })
}

/// A time-out's pill (`rounded-full bg-amber-500/15 px-2 py-1 text-xs font-bold`).
pub(super) fn timeout_pill(id: &str, text: String, p: &Palette, window: &Window) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded_full()
        .bg(gpui_kit::rgba(AMBER_500 << 8 | 0x26))
        .px(px(8.0))
        .py(px(4.0))
        .text_xs()
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(amber_text(p))
        .child(turning_hourglass(format!("{id}-hourglass"), window))
        .child(text)
}

/// What you may do to a member, as the web's `moderationFor`.
#[derive(Clone, Copy, Default)]
struct Can {
    nickname: bool,
    timeout: bool,
    kick: bool,
    ban: bool,
}

impl Can {
    fn any(self) -> bool {
        self.nickname || self.timeout || self.kick || self.ban
    }
}

/// A member on the page: who, what you may do, their roles (highest first), their color.
struct Shown {
    member: pb::Member,
    owner: bool,
    can: Can,
    roles: Vec<pb::Role>,
    color: Option<u32>,
    rank: i64,
}

impl ServerSettingsView {
    // ───────────────────────── Invites ─────────────────────────

    pub(super) fn invites_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let streamer = self.core.prefs().streamer_mode;
        let base = self
            .core
            .shared
            .read(|s| {
                s.instance(&self.key).map(|i| {
                    i.node
                        .as_ref()
                        .map(|n| n.public_url.clone())
                        .filter(|u| !u.is_empty())
                        .unwrap_or_else(|| self.core.api(&self.key).map(|a| a.url.clone()).unwrap_or_default())
                })
            })
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_owned();
        let (access, channels) = self.core.shared.read(|s| {
            let i = s.instance(&self.key);
            (
                i.map(|i| i.access(&self.server)).unwrap_or_default(),
                i.and_then(|i| i.channels.get(&self.server).cloned()).unwrap_or_default(),
            )
        });
        let manager = access.has(P::ManageServer);
        // Where "Create invite" leads: the server if you may invite to it, else the first channel you may.
        let target = if access.has(P::CreateInvite) {
            Some(String::new())
        } else {
            channels
                .iter()
                .find(|c| {
                    matches!(c.r#type(), pb::ChannelType::Text | pb::ChannelType::Announcement)
                        && access.has_in(&c.id, P::CreateInvite)
                })
                .map(|c| c.id.clone())
        };
        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div().max_w(px(448.0)).text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(
                    if manager { t("serversettings.invites.everyone") } else { t("serversettings.invites.own") },
                ),
            )
            .when_some(target, |el, channel| {
                el.child(
                    button("invite-make", t("serversettings.invites.create"), Some("plus"), Look::Primary, false, p)
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.emit(ServerSettingsEvent::Invite { channel: channel.clone() });
                            // The dialog makes the link; read the list again once it has.
                            cx.spawn(async move |this, cx| {
                                for wait in [1200u64, 3000, 8000] {
                                    cx.background_executor().timer(Duration::from_millis(wait)).await;
                                    if this.update(cx, |this, cx| this.load_invites(cx)).is_err() {
                                        break;
                                    }
                                }
                            })
                            .detach();
                            let _ = this;
                        })),
                )
            });
        let now = now_ms();
        let body: AnyElement = match &self.invites {
            _ if self.error.is_some() && self.invites.is_none() => {
                super::pages::problem(self.error.as_deref().unwrap_or_default(), p)
            }
            None => super::pages::shimmers(3, 64.0, radius_2xl(), p, window),
            Some((invites, people)) => {
                let live: Vec<&pb::Invite> = invites
                    .iter()
                    .filter(|i| {
                        (i.max_uses == 0 || i.uses < i.max_uses)
                            && i.expires_at.as_ref().is_none_or(|t| t.seconds * 1000 > now)
                    })
                    .collect();
                if live.is_empty() {
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
                            .p(px(40.0))
                            .child(motion::ambient(
                                div()
                                    .size(px(48.0))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(alpha(p.primary, 0.15))
                                    .text_color(p.primary)
                                    .child(icon("link").size(px(24.0))),
                                "invites-float",
                                Duration::from_millis(3000),
                                window,
                                |el, t| el.relative().top(px(-3.0 * (t * std::f32::consts::TAU).sin())),
                            ))
                            .child(div().font_weight(FontWeight::BOLD).child(t("serversettings.invites.none")))
                            .child(
                                div()
                                    .max_w(px(384.0))
                                    .text_center()
                                    .text_sm()
                                    .line_height(px(20.0))
                                    .text_color(p.muted_foreground)
                                    .child(t("serversettings.invites.noneHint")),
                            ),
                        "invites-empty",
                        Duration::ZERO,
                        8.0,
                    )
                    .into_any_element()
                } else {
                    let mut list = div().flex().flex_col().gap(px(8.0));
                    for (n, invite) in live.into_iter().enumerate() {
                        let channel = channels.iter().find(|c| c.id == invite.channel_id).map(|c| c.name.clone());
                        let row = self.invite_row(
                            invite,
                            people.get(&invite.inviter_id),
                            channel,
                            format!("{base}/invite/{}", invite.code),
                            streamer,
                            now,
                            p,
                            cx,
                        );
                        list = list.child(motion::once(
                            row,
                            SharedString::from(format!("invite-in-{}", invite.code)),
                            Duration::from_millis(420 + 30 * n.min(10) as u64),
                            move |el, t| {
                                let start = (n.min(10) as f32 * 30.0) / (420.0 + n.min(10) as f32 * 30.0);
                                let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                                let e = 1.0 - (1.0 - k).powi(3);
                                el.opacity(e).relative().left(px(-12.0 * (1.0 - e)))
                            },
                        ));
                    }
                    list.into_any_element()
                }
            }
        };
        div().flex().flex_col().gap(px(16.0)).child(head).child(body).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn invite_row(
        &self,
        invite: &pb::Invite,
        inviter: Option<&pb::User>,
        channel: Option<String>,
        link: String,
        streamer: bool,
        now: i64,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let code = invite.code.clone();
        let copied =
            self.copied.as_ref().is_some_and(|(c, at)| *c == invite.code && at.elapsed() < Duration::from_millis(1400));
        let hover_border = alpha(p.primary, 0.3);
        let who =
            div().flex().flex_1().min_w(px(192.0)).items_center().gap(px(10.0)).child(avatar(inviter, 36.0, p)).child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .child(inviter.map(user_name).unwrap_or_else(|| t("common.someone"))),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family("monospace")
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(if streamer { "••••••••".to_owned() } else { invite.code.clone() }),
                    ),
            );
        let place = div()
            .w(px(128.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_sm()
            .line_height(px(20.0))
            .text_color(p.muted_foreground)
            .child(icon(if invite.channel_id.is_empty() { "server" } else { "hash" }).size(px(14.0)))
            .child(div().truncate().child(if invite.channel_id.is_empty() {
                t("serversettings.invites.server")
            } else {
                channel.unwrap_or_else(|| t("serversettings.invites.deletedChannel"))
            }));
        let share = if invite.max_uses > 0 { invite.uses as f32 / invite.max_uses as f32 } else { 1.0 };
        let primary = p.primary;
        let uses = div()
            .w(px(96.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .child(div().font_weight(FontWeight::BOLD).child(invite.uses.to_string()))
                    .child(div().text_color(p.muted_foreground).child("/"))
                    .child(if invite.max_uses > 0 {
                        div().text_color(p.muted_foreground).child(invite.max_uses.to_string()).into_any_element()
                    } else {
                        icon("infinity").size(px(14.0)).text_color(p.muted_foreground).into_any_element()
                    }),
            )
            .child(div().h(px(4.0)).w_full().rounded_full().bg(p.muted).child(motion::once(
                div().h_full().rounded_full().bg(primary),
                SharedString::from(format!("invite-bar-{}-{}", invite.code, invite.uses)),
                Duration::from_millis(800),
                move |el, t| {
                    let e = 1.0 - (1.0 - t).powi(3);
                    el.w(gpui_kit::relative(share * e)).opacity(if share >= 1.0 && e > 0.0 { 0.25 } else { 1.0 })
                },
            )));
        let until = invite.expires_at.as_ref().map(|t| t.seconds * 1000);
        let soon = until.is_some_and(|u| u - now < 3_600_000);
        let expiry = div()
            .w(px(96.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_sm()
            .line_height(px(20.0))
            .text_color(if soon { gpui_kit::rgb(AMBER_500).into() } else { Hsla::from(p.muted_foreground) })
            .child(icon(if until.is_some() { "timer" } else { "infinity" }).size(px(14.0)))
            .child(match until {
                Some(u) => left(u - now),
                None => t("serversettings.shared.never"),
            });
        let (hover_bg, fg) = (p.muted, p.foreground);
        let success: Hsla = gpui_kit::rgb(EMERALD_500).into();
        let copy = div()
            .id(SharedString::from(format!("invite-copy-{code}")))
            .size(px(32.0))
            .rounded(radius_lg())
            .flex()
            .items_center()
            .justify_center()
            .text_color(if copied { success } else { p.muted_foreground.into() })
            .cursor_pointer()
            .when(!copied, |el| el.hover(move |s| s.bg(hover_bg).text_color(fg)))
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener({
                let code = code.clone();
                move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link.clone()));
                    this.copied = Some((code.clone(), Instant::now()));
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_millis(1450)).await;
                        let _ = this.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                    cx.notify();
                }
            }))
            .child(motion::once(
                icon(if copied { "check" } else { "copy" }).size(px(16.0)),
                SharedString::from(format!("invite-copy-{code}-{copied}")),
                Duration::from_millis(260),
                |el, t| el.opacity(t),
            ));
        let red = alpha(p.destructive, 0.1);
        let destructive = p.destructive;
        let revoke = div()
            .id(SharedString::from(format!("invite-revoke-{code}")))
            .size(px(32.0))
            .rounded(radius_lg())
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover(move |s| s.bg(red).text_color(destructive))
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(move |this, _, _, cx| this.revoke_invite(code.clone(), cx)))
            .child(icon("x").size(px(16.0)));
        list_row(p)
            .id(SharedString::from(format!("invite-row-{}", invite.code)))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(16.0))
            .gap_y(px(8.0))
            .p(px(12.0))
            .hover(move |s| s.border_color(hover_border))
            .child(who)
            .child(place)
            .child(uses)
            .child(expiry)
            .child(div().ml_auto().flex().items_center().gap(px(4.0)).child(copy).child(revoke))
    }

    fn revoke_invite(&mut self, code: String, cx: &mut Context<Self>) {
        if let Some((list, _)) = &mut self.invites {
            list.retain(|i| i.code != code);
        }
        cx.notify();
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.delete_invite(&key, &sid, &code).await }, |this, result, cx| {
            if let Err(err) = result {
                cx.emit(ServerSettingsEvent::Toast { icon: "circle-alert", title: err.message });
                this.load_invites(cx);
            }
            cx.notify();
        });
    }

    // ───────────────────────── Members ─────────────────────────

    pub(super) fn members_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let query = self.member_query.read(cx).value().trim().to_lowercase();
        let now = now_ms();
        let picked_role = self.pages.people.role.clone();
        let only_timed_out = self.pages.people.timed_out;
        let (mut shown, roles, timed_out, mine, me) = self.core.shared.read(|s| {
            let Some(i) = s.instance(&self.key) else { return (Vec::new(), Vec::new(), 0, Access::default(), None) };
            let owner_id = i.server(&self.server).map(|s| s.owner_id.clone()).unwrap_or_default();
            let mut roles = i.roles.get(&self.server).cloned().unwrap_or_default();
            roles.sort_by_key(|r| std::cmp::Reverse(r.position));
            let me = i.me.as_ref().map(|m| m.id.clone());
            let mine = i.access(&self.server);
            let members = i.members.get(&self.server).cloned().unwrap_or_default();
            let timed_out = members.iter().filter(|m| timed_out_until(m, now).is_some()).count();
            let picked = picked_role.as_ref().and_then(|id| roles.iter().find(|r| &r.id == id));
            let shown: Vec<Shown> = members
                .into_iter()
                .filter(|m| !only_timed_out || timed_out_until(m, now).is_some())
                .filter(|m| picked.is_none_or(|r| m.role_ids.contains(&r.id)))
                .filter(|m| {
                    query.is_empty()
                        || m.user.as_ref().is_some_and(|u| {
                            format!("{} {} {}", member_name(m), u.display_name, u.username)
                                .to_lowercase()
                                .contains(&query)
                        })
                })
                .map(|m| {
                    let uid = m.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
                    let standing = i.standing(&self.server, &uid);
                    let below = me.as_deref() != Some(uid.as_str()) && outranks(&mine, &standing);
                    let can = Can {
                        nickname: below && mine.has(P::ManageNicknames),
                        timeout: below && mine.has(P::TimeOutMembers),
                        kick: below && mine.has(P::KickMembers),
                        ban: below && mine.has(P::BanMembers),
                    };
                    let held: Vec<pb::Role> =
                        roles.iter().filter(|r| r.id != self.server && m.role_ids.contains(&r.id)).cloned().collect();
                    let color = held.iter().find_map(|r| r.color.map(|c| c as u32));
                    Shown { owner: uid == owner_id, can, color, rank: standing.rank, roles: held, member: m }
                })
                .collect();
            (shown, roles, timed_out, mine, me)
        });
        let _ = me;
        // Highest ranked first; the list is already by name within a rank.
        shown.sort_by_key(|m| std::cmp::Reverse(m.rank));
        let picked = picked_role.as_ref().and_then(|id| roles.iter().find(|r| &r.id == id)).cloned();

        // Filters: the search, a role, and all or timed out.
        let search = search_box(&self.member_query, p, window, cx).flex_1().min_w(px(224.0));
        let open = self.menu_open("members-role");
        let border_hover = alpha(p.primary, 0.4);
        let trigger =
            div()
                .id("members-role")
                .h(px(40.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(radius_xl())
                .border_1()
                .px(px(12.0))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .map(|el| match (&picked, open) {
                    (Some(_), _) => el.border_color(alpha(p.primary, 0.5)).bg(alpha(p.primary, 0.1)),
                    (None, true) => el.border_color(alpha(p.primary, 0.6)),
                    (None, false) => el.border_color(p.border).hover(move |s| s.border_color(border_hover)),
                })
                .child(match &picked {
                    Some(r) => role_dot(r.color.map(|c| c as u32), p).into_any_element(),
                    None => icon("funnel").size(px(16.0)).text_color(p.muted_foreground).into_any_element(),
                })
                .child(div().max_w(px(128.0)).truncate().child(
                    picked.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| t("serversettings.members.anyRole")),
                ));
        let mut items = vec![
            Item::action(t("serversettings.members.anyRole"), lead_icon("funnel", p), |this, _, _| {
                this.pages.people.role = None
            })
            .checked(picked_role.is_none()),
        ];
        for r in roles.iter().filter(|r| r.id != self.server) {
            let id = r.id.clone();
            items.push(
                Item::action(
                    r.name.clone(),
                    Some(role_dot(r.color.map(|c| c as u32), p).into_any_element()),
                    move |this, _, _| this.pages.people.role = Some(id.clone()),
                )
                .checked(picked_role.as_deref() == Some(r.id.as_str())),
            );
        }
        let role_menu = self.dropdown("members-role".into(), trigger, items, true, 208.0, 44.0, p, cx);
        let shows = crate::ui::settings_controls::segmented(
            "members-show",
            vec![
                (t("serversettings.members.all"), None),
                (
                    if timed_out > 0 {
                        t_with("serversettings.members.timedOutCount", &[("count", Arg::Num(timed_out as i64))])
                    } else {
                        t("serversettings.members.timedOut")
                    },
                    None,
                ),
            ],
            usize::from(only_timed_out),
            if timed_out > 0 { 128.0 } else { 104.0 },
            p,
            window,
            cx,
            |this: &mut Self, n, cx| {
                this.pages.people.timed_out = n == 1;
                cx.notify();
            },
        );
        let filters = div().flex().flex_wrap().items_center().gap(px(8.0)).child(search).child(role_menu).child(shows);

        let count = t_with("serversettings.shared.members", &[("count", Arg::Num(shown.len() as i64))]);
        let mut list = div().flex().flex_col().gap(px(6.0));
        let n_shown = shown.len();
        for (n, s) in shown.into_iter().enumerate() {
            list = list.child(self.member_row(s, n, &roles, &mine, now, p, window, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(filters)
            .child(count_line("users", count, p))
            .child(list)
            .when(n_shown == 0, |el| {
                el.child(motion::rise(
                    div()
                        .py(px(32.0))
                        .text_center()
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(if only_timed_out && query.is_empty() && picked.is_none() {
                            t("serversettings.members.noneTimedOut")
                        } else {
                            t("serversettings.shared.nobodyMatches")
                        }),
                    "members-nobody",
                    Duration::ZERO,
                    8.0,
                ))
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn member_row(
        &mut self,
        s: Shown,
        n: usize,
        roles: &[pb::Role],
        mine: &Access,
        now: i64,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(user) = s.member.user.clone() else { return div().into_any_element() };
        let uid = user.id.clone();
        let name = member_name(&s.member);
        let joined_ms = s.member.joined_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
        let day = crate::ui::text::day(joined_ms);
        let joined =
            if day == t("common.time.today") || day == t("common.time.yesterday") { day.to_lowercase() } else { day };
        let until = timed_out_until(&s.member, now);
        let developer = self.core.prefs().developer_mode;
        let role_colors = self.core.prefs().role_colors;
        let colored = s.color.is_some() && role_colors == crate::core::config::RoleColors::Names;
        let name_color: Hsla = match s.color {
            Some(c) if colored => gpui_kit::rgb(c).into(),
            _ => crate::ui::widgets::name_tint(&uid, p),
        };
        let title = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .child(div().truncate().font_weight(FontWeight::BOLD).text_color(name_color).child(name.clone()))
            .when(s.color.is_some() && role_colors == crate::core::config::RoleColors::Beside, |el| {
                el.child(
                    div()
                        .size(px(8.0))
                        .flex_none()
                        .rounded_full()
                        .border_2()
                        .border_color(p.background)
                        .bg(gpui_kit::rgb(s.color.unwrap_or_default())),
                )
            })
            .when(s.owner, |el| el.child(icon("crown").size(px(14.0)).text_color(gpui_kit::rgb(AMBER_400))));
        let line = div().truncate().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(t_with(
            "serversettings.members.line",
            &[("username", Arg::Str(&user.username)), ("date", Arg::Str(&joined))],
        ));
        let chips = self.member_roles(&s.member, &s.roles, roles, mine, p, cx);
        let hover_border = alpha(p.primary, 0.3);
        let hover_bg = alpha(p.muted, 0.4);
        let mut row = list_row(p)
            .id(SharedString::from(format!("member-row-{uid}")))
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(10.0))
            .pr(px(8.0))
            .hover(move |s| s.border_color(hover_border).bg(hover_bg))
            .child(div().self_start().child(avatar(Some(&user), 40.0, p)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(line)
                    .when_some(chips, |el, c| el.child(div().mt(px(6.0)).child(c))),
            );
        if let Some(until) = until {
            let pill = timeout_pill(&format!("member-{uid}"), left(until - now), p, window);
            let ends =
                t_with("serversettings.members.timedOutUntil", &[("time", Arg::Str(&crate::ui::text::when(until)))]);
            let hover = gpui_kit::rgba(AMBER_500 << 8 | 0x40);
            let target = uid.clone();
            row = row.child(motion::once(
                pill.id(SharedString::from(format!("member-pill-{uid}")))
                    .tooltip(move |w, cx| crate::ui::overlay::Tip::new(ends.clone()).build(w, cx))
                    .when(s.can.timeout, |el| {
                        el.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ServerSettingsEvent::Moderate {
                                user_id: target.clone(),
                                action: Action::TimeOut(3_600),
                            })
                        }))
                    }),
                SharedString::from(format!("member-pill-in-{uid}")),
                Duration::from_millis(300),
                |el, t| el.opacity(t),
            ));
        }
        if s.can.any() || developer {
            row = row.child(self.member_menu(&uid, &name, s.can, until.is_some(), developer, p, cx));
        }
        motion::rise(
            row,
            SharedString::from(format!("member-in-{uid}")),
            Duration::from_millis(20 * n.min(14) as u64),
            10.0,
        )
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn member_menu(
        &self,
        uid: &str,
        name: &str,
        can: Can,
        timed_out: bool,
        developer: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("member-menu-{uid}");
        let open = self.menu_open(&id);
        let (hover_bg, fg) = (p.muted, p.foreground);
        let trigger = div()
            .id(SharedString::from(id.clone()))
            .size(px(36.0))
            .flex_none()
            .rounded(radius_xl())
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .map(|el| {
                if open {
                    el.bg(p.muted).text_color(p.foreground)
                } else {
                    el.text_color(p.muted_foreground).hover(move |s| s.bg(hover_bg).text_color(fg))
                }
            })
            .child(icon("ellipsis").size(px(16.0)));
        let _ = name;
        let punish = can.timeout || can.kick || can.ban;
        let mut items = Vec::new();
        let act = |action: Action, uid: String| {
            move |_: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                cx.emit(ServerSettingsEvent::Moderate { user_id: uid.clone(), action })
            }
        };
        if can.nickname {
            let target = uid.to_owned();
            items.push(Item::action(
                t("serversettings.members.changeNickname"),
                lead_icon("pencil", p),
                move |this, window, cx| this.start_nickname(&target, window, cx),
            ));
        }
        if can.nickname && punish {
            items.push(Item::Separator);
        }
        if can.timeout {
            items.push(Item::action(
                if timed_out { t("serversettings.members.changeTimeout") } else { t("serversettings.members.timeOut") },
                lead_icon("hourglass", p),
                act(Action::TimeOut(3_600), uid.to_owned()),
            ));
        }
        let red = |g: &str| Some(icon(g).size(px(16.0)).text_color(p.destructive).into_any_element());
        if can.kick {
            items.push(
                Item::action(t("serversettings.members.kick"), red("door-open"), act(Action::Kick, uid.to_owned()))
                    .danger(),
            );
        }
        if can.ban {
            items.push(
                Item::action(t("serversettings.members.ban"), red("gavel"), act(Action::Ban(0), uid.to_owned()))
                    .danger(),
            );
        }
        if developer && can.any() {
            items.push(Item::Separator);
        }
        if developer {
            let target = uid.to_owned();
            items.push(Item::action(
                t_with("common.copyThing", &[("what", Arg::Str(&t("common.copy.userId")))]),
                lead_icon("fingerprint-pattern", p),
                move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(target.clone()));
                    cx.emit(ServerSettingsEvent::Toast {
                        icon: "check",
                        title: t_with("common.copied", &[("what", Arg::Str(&t("common.copy.userId")))]),
                    });
                },
            ));
        }
        self.dropdown(id, trigger, items, true, 208.0, 40.0, p, cx)
    }

    /// Someone's roles as chips (the web's compact `MemberRoles`): with Manage Roles, roles below
    /// yours come off from their dot, and a + gives more.
    fn member_roles(
        &self,
        member: &pb::Member,
        held: &[pb::Role],
        roles: &[pb::Role],
        mine: &Access,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let manage = mine.has(P::ManageRoles);
        let assignable: Vec<&pb::Role> =
            roles.iter().filter(|r| r.id != self.server && mine.above(r.position)).collect();
        if held.is_empty() && !(manage && !assignable.is_empty()) {
            return None;
        }
        let uid = member.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        let mut chips = div().flex().flex_wrap().gap(px(4.0));
        for role in held {
            let removable = manage && mine.above(role.position);
            let busy = self.pages.people.toggling.contains(&(uid.clone(), role.id.clone()));
            let group = SharedString::from(format!("role-chip-{uid}-{}", role.id));
            let dot: AnyElement = if removable {
                let (u, r) = (uid.clone(), role.id.clone());
                div()
                    .id(SharedString::from(format!("role-off-{uid}-{}", role.id)))
                    .relative()
                    .size(px(12.0))
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_role(&u, &r, false, cx)))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .group_hover(group.clone(), |s| s.opacity(0.0))
                            .child(role_dot(role.color.map(|c| c as u32), p).size(px(12.0))),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(0.0)
                            .text_color(p.destructive)
                            .group_hover(group.clone(), |s| s.opacity(1.0))
                            .child(icon("x").size(px(12.0))),
                    )
                    .into_any_element()
            } else {
                role_dot(role.color.map(|c| c as u32), p).size(px(12.0)).into_any_element()
            };
            chips = chips.child(motion::once(
                div()
                    .group(group)
                    .h(px(24.0))
                    .max_w_full()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded_full()
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.background, 0.7))
                    .pl(px(6.0))
                    .pr(px(8.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .when(busy, |el| el.opacity(0.5))
                    .child(dot)
                    .child(div().truncate().child(role.name.clone())),
                SharedString::from(format!("role-chip-in-{uid}-{}", role.id)),
                Duration::from_millis(220),
                |el, t| el.opacity(t),
            ));
        }
        if manage && !assignable.is_empty() {
            let id = format!("member-roles-{uid}");
            let open = self.menu_open(&id);
            let (hover_border, primary) = (alpha(p.primary, 0.5), p.primary);
            let trigger = div()
                .id(SharedString::from(id.clone()))
                .size(px(24.0))
                .rounded_full()
                .border_1()
                .border_dashed()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .map(|el| {
                    if open {
                        el.border_color(p.border).text_color(p.primary)
                    } else {
                        el.border_color(p.border)
                            .text_color(p.muted_foreground)
                            .hover(move |s| s.border_color(hover_border).text_color(primary))
                    }
                })
                .child(icon("plus").size(px(14.0)));
            let mut items = vec![Item::Label(t("workspace.roles.assignable"))];
            for role in assignable {
                let on = member.role_ids.contains(&role.id);
                let (u, r) = (uid.clone(), role.id.clone());
                items.push(
                    Item::action(
                        role.name.clone(),
                        Some(role_dot(role.color.map(|c| c as u32), p).size(px(12.0)).into_any_element()),
                        move |this, _, cx| this.toggle_role(&u, &r, !on, cx),
                    )
                    .checked(on)
                    .stay(),
                );
            }
            chips = chips.child(self.dropdown(id, trigger, items, false, 224.0, 28.0, p, cx));
        }
        Some(chips.into_any_element())
    }

    fn toggle_role(&mut self, user: &str, role: &str, give: bool, cx: &mut Context<Self>) {
        let k = (user.to_owned(), role.to_owned());
        if !self.pages.people.toggling.insert(k.clone()) {
            return;
        }
        let (core, key, sid, user, role) =
            (self.core.clone(), self.key.clone(), self.server.clone(), user.to_owned(), role.to_owned());
        self.run(cx, async move { core.set_member_role(&key, &sid, &user, &role, give).await }, move |this, r, cx| {
            this.pages.people.toggling.remove(&k);
            if let Err(err) = r {
                cx.emit(ServerSettingsEvent::Toast { icon: "circle-alert", title: err.message });
            }
            cx.notify();
        });
        cx.notify();
    }

    fn start_nickname(&mut self, user: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.members.get(&self.server))
                .and_then(|l| l.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user)))
                .map(|m| (m.nickname.clone(), m.user.as_ref().map(user_name).unwrap_or_default()))
        });
        let Some((nickname, plain)) = current else { return };
        let people = &mut self.pages.people;
        people.nickname = Some(user.to_owned());
        people.nick_error = None;
        people.nick_busy = false;
        people.nick.update(cx, |s, cx| {
            s.set_value(nickname, window, cx);
            s.set_placeholder(plain, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn save_nickname(&mut self, cx: &mut Context<Self>) {
        let Some(user) = self.pages.people.nickname.clone() else { return };
        let nickname = self.pages.people.nick.read(cx).value().trim().to_owned();
        self.pages.people.nick_busy = true;
        self.pages.people.nick_error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        self.run(cx, async move { core.set_member_nickname(&key, &sid, &user, &nickname).await }, |this, r, cx| {
            let people = &mut this.pages.people;
            people.nick_busy = false;
            match r {
                Ok(()) => people.nickname = None,
                Err(err) => people.nick_error = Some(err.message),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// The nickname dialog over the settings (the web's `ModerateDialog` for "nickname").
    pub(super) fn nickname_dialog(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let user = self.pages.people.nickname.clone()?;
        let member = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.members.get(&self.server))
                .and_then(|l| l.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user)))
                .cloned()
        })?;
        let name = member_name(&member);
        let people = &self.pages.people;
        let busy = people.nick_busy;
        let (hover_bg, fg) = (p.muted, p.foreground);
        let who =
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .rounded(radius_2xl())
                .bg(alpha(p.muted, 0.6))
                .p(px(12.0))
                .child(avatar(member.user.as_ref(), 40.0, p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().font_weight(FontWeight::BOLD).child(name.clone()))
                        .child(div().truncate().text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(
                            format!("@{}", member.user.as_ref().map(|u| u.username.as_str()).unwrap_or_default()),
                        )),
                );
        let field = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(super::pages::label(&t("workspace.moderate.nickname")).font_weight(FontWeight::BOLD))
            .child(boxed(Input::new(&people.nick).appearance(false), 44.0, focused(&people.nick, window, cx), p));
        let card = div()
            .id("nickname-dialog")
            .relative()
            .w(px(448.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(24.0))
            .shadow(crate::ui::settings_controls::shadow_xl())
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.save_nickname(cx);
                }
            }))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .mb(px(4.0))
                    .pr(px(32.0))
                    .child(
                        div()
                            .text_xl()
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t_with("workspace.moderate.title.nickname", &[("name", Arg::Str(&name))])),
                    )
                    .child(
                        div()
                            .mt(px(4.0))
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("workspace.moderate.about.nickname")),
                    ),
            )
            .child(who)
            .child(field)
            .when_some(people.nick_error.clone(), |el, e| {
                el.child(div().text_sm().text_color(p.destructive).child(crate::ui::instance_home::capitalized(&e)))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        button("nickname-cancel", t("common.cancel"), None, Look::Ghost, false, p)
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pages.people.nickname = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        button(
                            "nickname-save",
                            t("workspace.moderate.submit.save"),
                            Some("pencil"),
                            Look::Primary,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .when(busy, |el| el.opacity(0.5))
                        .when(!busy, |el| el.on_click(cx.listener(|this, _, _, cx| this.save_nickname(cx)))),
                    ),
            )
            .child(
                div()
                    .id("nickname-close")
                    .absolute()
                    .top(px(16.0))
                    .right(px(16.0))
                    .size(px(32.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg).text_color(fg))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pages.people.nickname = None;
                        cx.notify();
                    }))
                    .child(icon("x").size(px(16.0))),
            );
        Some(
            div()
                .id("nickname-scrim")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.pages.people.nickname = None;
                        cx.notify();
                    }),
                )
                .child(crate::ui::motion::sheet_up(card, "nickname-dialog-in"))
                .into_any_element(),
        )
    }

    // ───────────────────────── Bans ─────────────────────────

    pub(super) fn bans_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let (Some(e), None) = (&self.error, &self.bans) {
            return super::pages::problem(e, p);
        }
        let Some((bans, moderators)) = self.bans.clone() else {
            return super::pages::shimmers(3, 64.0, radius_2xl(), p, window);
        };
        if bans.is_empty() {
            return motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .py(px(48.0))
                    .child(motion::once(
                        div()
                            .size(px(64.0))
                            .rounded(radius_3xl())
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.primary, 0.1))
                            .text_color(p.primary)
                            .child(icon("shield-check").size(px(32.0))),
                        "bans-empty-shield",
                        Duration::from_millis(600),
                        |el, t| {
                            let k = ((t - 0.15) / 0.85).clamp(0.0, 1.0);
                            let e = 1.0 - (1.0 - k).powi(3) * (1.0 + 2.0 * k * (1.0 - k));
                            el.opacity(k.min(1.0)).relative().top(px(-10.0 * (1.0 - e)))
                        },
                    ))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("serversettings.bans.none")))
                    .child(
                        div()
                            .max_w(px(320.0))
                            .text_center()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("serversettings.bans.noneHint")),
                    ),
                "bans-empty",
                Duration::ZERO,
                8.0,
            )
            .into_any_element();
        }
        let query = self.pages.people.bans_query.read(cx).value().trim().to_lowercase();
        let shown: Vec<&pb::Ban> = bans
            .iter()
            .filter(|b| {
                query.is_empty() || {
                    let u = b.user.as_ref();
                    format!(
                        "{} {} {}",
                        u.map(user_name).unwrap_or_default(),
                        u.map(|u| u.username.as_str()).unwrap_or_default(),
                        b.reason
                    )
                    .to_lowercase()
                    .contains(&query)
                }
            })
            .collect();
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (n, ban) in shown.iter().enumerate() {
            list = list.child(self.ban_row(ban, &moderators, n, p, window, cx));
        }
        let count = t_with("serversettings.bans.count", &[("count", Arg::Num(bans.len() as i64))]);
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(search_box(&self.pages.people.bans_query, p, window, cx))
            .child(count_line("gavel", count, p))
            .child(list)
            .when(shown.is_empty(), |el| {
                el.child(
                    div()
                        .py(px(24.0))
                        .text_center()
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(t("serversettings.shared.nobodyMatches")),
                )
            })
            .into_any_element()
    }

    fn ban_row(
        &self,
        ban: &pb::Ban,
        moderators: &crate::core::server_admin::People,
        n: usize,
        p: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let user = ban.user.clone();
        let uid = user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        let at = ban.created_at.as_ref().map(|t| t.seconds * 1000).unwrap_or_default();
        let date = crate::ui::text::day(at);
        let by = match moderators.get(&ban.banned_by_id) {
            Some(m) => {
                t_with("serversettings.bans.bannedBy", &[("name", Arg::Str(&user_name(m))), ("date", Arg::Str(&date))])
            }
            None => t_with("serversettings.bans.bannedBySomeone", &[("date", Arg::Str(&date))]),
        };
        let lifting = self.pages.people.lifting.as_deref() == Some(uid.as_str());
        let group = SharedString::from(format!("ban-{uid}"));
        let face = div()
            .relative()
            .flex_none()
            .child(
                div()
                    .child(avatar(user.as_ref(), 40.0, p))
                    .opacity(0.75)
                    .group_hover(group.clone(), |s| s.opacity(1.0)),
            )
            .child(
                div()
                    .absolute()
                    .right(px(-4.0))
                    .bottom(px(-4.0))
                    .size(px(20.0))
                    .rounded_full()
                    .border_2()
                    .border_color(p.background)
                    .bg(p.destructive)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(gpui_kit::white())
                    .child(icon("gavel").size(px(12.0))),
            );
        let reason: AnyElement = if ban.reason.is_empty() {
            div().italic().text_color(p.muted_foreground).child(t("serversettings.bans.noReason")).into_any_element()
        } else {
            div().child(ban.reason.clone()).into_any_element()
        };
        let info = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(4.0))
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .child(user.as_ref().map(user_name).unwrap_or_default()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(format!("@{}", user.as_ref().map(|u| u.username.as_str()).unwrap_or_default())),
                    ),
            )
            .child(div().text_sm().line_height(px(20.0)).child(reason))
            .child(div().mt(px(2.0)).text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(by));
        let unban = button(
            SharedString::from(format!("unban-{uid}")),
            t("serversettings.bans.unban"),
            None,
            Look::Outline,
            true,
            p,
        )
        .rounded(radius_xl())
        .when(lifting, |el| el.opacity(0.5).child(spinner(format!("unban-spin-{uid}"), 16.0, window)))
        .when(!lifting, |el| el.child(icon("undo").size(px(16.0))))
        .when(!lifting, |el| {
            let (uid, name) = (uid.clone(), user.as_ref().map(user_name).unwrap_or_default());
            el.on_click(cx.listener(move |this, _, _, cx| this.unban(uid.clone(), name.clone(), cx)))
        });
        // The icon goes before the words, as the web's button has it.
        let unban = div().flex_none().child(unban.flex_row_reverse());
        motion::rise(
            list_row(p)
                .group(group)
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(12.0))
                .child(face)
                .child(info)
                .child(unban),
            SharedString::from(format!("ban-in-{uid}")),
            Duration::from_millis(30 * n.min(12) as u64),
            10.0,
        )
        .into_any_element()
    }

    fn unban(&mut self, uid: String, name: String, cx: &mut Context<Self>) {
        self.pages.people.lifting = Some(uid.clone());
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let gone = uid.clone();
        self.run(cx, async move { core.unban(&key, &sid, &uid).await }, move |this, result, cx| {
            this.pages.people.lifting = None;
            match result {
                Ok(()) => {
                    if let Some((list, _)) = &mut this.bans {
                        list.retain(|b| b.user.as_ref().is_none_or(|u| u.id != gone));
                    }
                    cx.emit(ServerSettingsEvent::Toast {
                        icon: "undo",
                        title: t_with("serversettings.bans.unbanned", &[("name", Arg::Str(&name))]),
                    });
                }
                Err(err) => cx.emit(ServerSettingsEvent::Toast { icon: "circle-alert", title: err.message }),
            }
            cx.notify();
        });
        cx.notify();
    }
}

/// A member's name in the server: their nickname, else their own.
fn member_name(m: &pb::Member) -> String {
    if m.nickname.is_empty() { m.user.as_ref().map(user_name).unwrap_or_default() } else { m.nickname.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_left_reads_like_the_web() {
        assert_eq!(left(42_000), "0:42");
        assert_eq!(left(12 * 60_000 + 5_000), "12:05");
        assert_eq!(left(3 * 3_600_000 + 12 * 60_000), "3h 12m");
        assert_eq!(left(4 * 86_400_000 + 2 * 3_600_000), "4d 2h");
    }
}
