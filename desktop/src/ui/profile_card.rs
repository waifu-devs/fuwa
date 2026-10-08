//! Someone's profile card, opening beside whatever you clicked on them (a
//! name or picture in chat, a row in the member list, a friend), like the
//! web app's `ProfilePopover.tsx` and `ProfileCard.tsx`: their banner,
//! picture with their dot, status, names, roles (`MemberRoles.tsx`), what
//! they're doing, about me and dates, then Message, the friend buttons and
//! moderation under it, each in a box of its own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::component::input::TextareaState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, AnyElement, AppContext as _, Bounds, BoxShadow, Context, Entity, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point, px, rgb,
};

use crate::core::dms::{DmStatus, now_ms};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::moderation::{Action, timed_out_until};
use crate::pb::{self, Permission as P};
use crate::ui::app::{Dialog, FuwaApp};
use crate::ui::motion;
use crate::ui::profile_effect::EffectView;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_md, radius_xl};
use crate::ui::widgets::{app_badge, avatar, icon, is_agent, pal};

/// The card's width: the web's `w-[19rem]`.
pub const WIDTH: f32 = 304.0;
/// Between the card and what opened it (`sideOffset`), and from the window's edges (`collisionPadding`).
const OFFSET: f32 = 10.0;
const EDGE: f32 = 12.0;
/// How long a profile may take before the card holds a place for the bio.
const SLOW: Duration = Duration::from_millis(500);
/// Marks remembered at most; the oldest go first.
const MARKS: usize = 600;

/// Which side of what was clicked the card opens on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Right,
    Left,
}

struct Mark {
    user: String,
    side: Side,
    bounds: Bounds<Pixels>,
    n: u64,
}

thread_local! {
    /// Where each card opener was last drawn, so the card can open beside the one clicked.
    static PLACES: RefCell<(u64, Vec<Mark>)> = const { RefCell::new((0, Vec::new())) };
}

fn remember(user: &str, side: Side, bounds: Bounds<Pixels>) {
    PLACES.with(|places| {
        let (n, marks) = &mut *places.borrow_mut();
        *n += 1;
        if let Some(m) = marks.iter_mut().find(|m| m.user == user && m.bounds == bounds) {
            m.n = *n;
            m.side = side;
            return;
        }
        if marks.len() >= MARKS {
            let oldest = marks.iter().enumerate().min_by_key(|(_, m)| m.n).map(|(i, _)| i).unwrap_or(0);
            marks.swap_remove(oldest);
        }
        marks.push(Mark { user: user.to_owned(), side, bounds, n: *n });
    });
}

/// Put inside anything that opens someone's card (it must not clip it away):
/// it notes where that is each time it's drawn, so the card opens beside it.
pub fn mark(user_id: &str, side: Side) -> impl IntoElement {
    let user = user_id.to_owned();
    canvas(move |bounds, _, _| remember(&user, side, bounds), |_, _, _, _| {}).absolute().inset_0()
}

/// Where the card for `user` goes: beside the opener under the pointer, else
/// the one drawn last, else the pointer itself.
fn anchor_for(user: &str, mouse: Point<Pixels>) -> (Bounds<Pixels>, Side) {
    PLACES.with(|places| {
        let (_, marks) = &*places.borrow();
        let mine = marks.iter().filter(|m| m.user == user);
        let under = mine.clone().filter(|m| m.bounds.contains(&mouse)).max_by_key(|m| m.n);
        under
            .or_else(|| mine.max_by_key(|m| m.n))
            .map(|m| (m.bounds, m.side))
            .unwrap_or((Bounds::new(mouse, gpui_kit::size(px(1.0), px(1.0))), Side::Right))
    })
}

/// What the cards and the moderation dialog keep between frames.
pub struct People {
    /// What the open card sits beside.
    anchor: Option<(Bounds<Pixels>, Side)>,
    opened: Instant,
    /// Profiles seen this session, shown at once while a fresh one loads.
    profiles: HashMap<String, pb::Profile>,
    /// The roles menu under the card's +, at this point.
    pub roles_menu: Option<Point<Pixels>>,
    /// A role being given or taken.
    busy_role: Option<String>,
    /// When the card was last closed by a click outside it, and whose it was:
    /// a click on the same name closes it rather than opening it again.
    closed: Option<(String, Instant)>,
    /// The moderation dialog's reason.
    pub reason: Entity<TextareaState>,
    /// The dialog the reason box was last emptied and focused for.
    pub reason_for: Option<String>,
    effect: Entity<EffectView>,
}

impl People {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> Self {
        Self {
            anchor: None,
            opened: Instant::now(),
            profiles: HashMap::new(),
            roles_menu: None,
            busy_role: None,
            closed: None,
            reason: cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6)),
            reason_for: None,
            effect: cx.new(|_| EffectView::new()),
        }
    }
}

/// The web's `shadow-md`.
pub(crate) fn shadow_md() -> Vec<BoxShadow> {
    let c = gpui_kit::hsla(0.0, 0.0, 0.0, 0.1);
    vec![
        BoxShadow {
            color: c,
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(6.0),
            spread_radius: px(-1.0),
            inset: false,
        },
        BoxShadow {
            color: c,
            offset: point(px(0.0), px(2.0)),
            blur_radius: px(4.0),
            spread_radius: px(-2.0),
            inset: false,
        },
    ]
}

/// The web's `.shine`: a soft white band sweeping across a button of `width`
/// now and then (1.5 seconds of every 7, starting 1.2 seconds in).
fn glint(id: &'static str, width: f32, window: &Window) -> AnyElement {
    let band = width * 0.4;
    let white = |a: f32| gpui_kit::hsla(0.0, 0.0, 1.0, a);
    let half = |from: f32, to: f32| {
        div().h_full().w(px(band * 0.5)).bg(gpui_kit::linear_gradient(
            100.0,
            gpui_kit::linear_color_stop(white(from), 0.0),
            gpui_kit::linear_color_stop(white(to), 1.0),
        ))
    };
    let el = div().absolute().top_0().bottom_0().w(px(band)).flex().child(half(0.0, 0.45)).child(half(0.45, 0.0));
    motion::ambient(el, id, Duration::from_millis(7000), window, move |el, t| {
        // Where in its 7 seconds the glint is, 1.2 seconds behind the clock.
        let k = ((t * 7.0 - 1.2).rem_euclid(7.0) / 7.0) / 0.22;
        let k = if k < 1.0 { 0.5 - 0.5 * (k * std::f32::consts::PI).cos() } else { 1.0 };
        el.left(px(band * (-1.2 + 4.4 * k)))
    })
}

/// "Oct 7, 2026".
fn day(ms: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local.timestamp_millis_opt(ms).single().map(|d| d.format("%b %-d, %Y").to_string()).unwrap_or_default()
}

/// The small capitals over each part of a card (`text-[0.7rem] font-extrabold tracking-wide uppercase`).
pub(crate) fn caps(text: &str, p: &Palette) -> gpui_kit::Div {
    div()
        .text_size(px(11.2))
        .line_height(px(16.8))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .child(tracked(text.to_uppercase(), WIDE))
}

/// A box under the card (`mt-2 rounded-2xl border bg-popover p-1.5 shadow-lg`), rising in after `delay` ms.
fn under_box(p: &Palette) -> gpui_kit::Div {
    div()
        .mt(px(8.0))
        .rounded(radius_2xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .p(px(6.0))
        .shadow(crate::ui::settings_controls::shadow_lg())
}

struct Facts {
    user: Option<pb::User>,
    member: Option<pb::Member>,
    owner: bool,
    me: bool,
    presence: Option<Option<pb::Presence>>,
    can_message: bool,
    can_friend: bool,
    /// Roles they hold, highest first, and whether you may take each.
    held: Vec<(pb::Role, bool)>,
    /// Roles you may hand out, highest first, when you have Manage Roles.
    assignable: Vec<pb::Role>,
    moderation: Vec<P>,
    /// The decoration around their avatar here, its picture's link (docs/profile-items.md).
    decoration: Option<String>,
}

impl FuwaApp {
    /// Called as a card opens: notes what it sits beside. False when the
    /// click was the one that just closed the same card, which stays closed.
    pub(crate) fn card_opening(&mut self, user_id: &str, window: &mut Window) -> bool {
        if self.people.closed.take().is_some_and(|(u, at)| u == user_id && at.elapsed() < Duration::from_millis(300)) {
            return false;
        }
        self.people.anchor = Some(anchor_for(user_id, window.mouse_position()));
        self.people.opened = Instant::now();
        self.people.roles_menu = None;
        true
    }

    fn close_card_outside(&mut self, user_id: &str, cx: &mut Context<Self>) {
        if self.people.roles_menu.is_some() {
            return;
        }
        if matches!(&self.dialog, Some(Dialog::Profile { user_id: u, .. }) if u == user_id) {
            self.people.closed = Some((user_id.to_owned(), Instant::now()));
            self.dialog = None;
            self.close_dialog(cx);
        }
    }

    fn card_facts(&self, key: &str, user_id: &str, server: Option<&str>) -> Facts {
        self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else {
                return Facts {
                    user: None,
                    member: None,
                    owner: false,
                    me: false,
                    presence: None,
                    can_message: false,
                    can_friend: false,
                    held: Vec::new(),
                    assignable: Vec::new(),
                    moderation: Vec::new(),
                    decoration: None,
                };
            };
            let member = server.and_then(|sid| {
                i.members.get(sid)?.iter().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id)).cloned()
            });
            let user = member.as_ref().and_then(|m| m.user.clone()).or_else(|| i.users.get(user_id).cloned());
            let me = i.me.as_ref().is_some_and(|m| m.id == user_id);
            let agent = is_agent(user.as_ref());
            let owner = server.and_then(|sid| i.server(sid)).is_some_and(|sv| sv.owner_id == user_id);
            let (held, assignable, moderation) = match (server, &member) {
                (Some(sid), Some(m)) => {
                    let access = i.access(sid);
                    let manage = access.has(P::ManageRoles);
                    let mut roles: Vec<pb::Role> =
                        i.roles.get(sid).into_iter().flatten().filter(|r| r.id != sid).cloned().collect();
                    roles.sort_by_key(|r| std::cmp::Reverse(r.position));
                    let held = roles
                        .iter()
                        .filter(|r| m.role_ids.contains(&r.id))
                        .map(|r| (r.clone(), manage && access.above(r.position)))
                        .collect();
                    let assignable = if manage {
                        roles.iter().filter(|r| access.above(r.position)).cloned().collect()
                    } else {
                        Vec::new()
                    };
                    (held, assignable, i.can_moderate(sid, user_id))
                }
                _ => (Vec::new(), Vec::new(), Vec::new()),
            };
            let decoration = i.decoration_url(server, member.as_ref(), user.as_ref()).map(str::to_owned);
            Facts {
                decoration,
                can_message: !me && !agent && matches!(i.dms.status, DmStatus::Ready | DmStatus::Starting),
                can_friend: !me && !agent && i.friends.status == crate::core::friends::FriendsStatus::Ready,
                presence: i.people.as_ref().map(|people| people.get(user_id).cloned()),
                user,
                member,
                owner,
                me,
                held,
                assignable,
                moderation,
            }
        })
    }

    /// The open card, beside what opened it.
    pub(crate) fn render_profile_card(
        &mut self,
        key: &str,
        user_id: &str,
        server: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let f = self.card_facts(key, user_id, server);
        let cache_key = format!("{key}|{user_id}");
        if let Some(fresh) = self.profile.clone().filter(|pr| pr.user.as_ref().is_some_and(|u| u.id == user_id)) {
            self.people.profiles.insert(cache_key.clone(), fresh);
        }
        let profile = self.people.profiles.get(&cache_key).cloned();
        let fresh = self.profile.as_ref().is_some_and(|pr| pr.user.as_ref().is_some_and(|u| u.id == user_id));
        let slow = !fresh && self.people.opened.elapsed() >= SLOW;
        if !fresh && !slow {
            // Look again once it's slow, to hold a place for the bio.
            let wait = SLOW.saturating_sub(self.people.opened.elapsed()) + Duration::from_millis(20);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let user = profile.as_ref().and_then(|pr| pr.user.clone()).or(f.user.clone());
        let loading = slow && profile.is_none();

        let card = self.card_body(key, user_id, user.as_ref(), profile.as_ref(), &f, loading, window, cx);
        let mut column = div()
            .id("profile-card")
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener({
                let uid = user_id.to_owned();
                move |this, _, _, cx| this.close_card_outside(&uid, cx)
            }))
            .child(card);
        if f.can_message {
            let (k, uid) = (key.to_owned(), user_id.to_owned());
            let fg = p.primary_foreground;
            column = column.child(motion::rise(
                under_box(&p).child(
                    div()
                        .id("profile-message")
                        .group("profile-message")
                        .relative()
                        .overflow_hidden()
                        .h(px(36.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .rounded(radius_xl())
                        .bg(p.primary)
                        .text_color(fg)
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.92))
                        .active(|s| s.top(px(1.0)))
                        .child(glint("profile-message-glint", WIDTH - 24.0, window))
                        .child(icon("message-circle").size(px(16.0)))
                        .child(t("workspace.popover.message"))
                        .child(div().opacity(0.8).child(icon("lock-keyhole").size(px(14.0))))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.dialog = None;
                            this.message_person(k.clone(), uid.clone(), window, cx);
                        })),
                ),
                "profile-message-in",
                Duration::from_millis(50),
                6.0,
            ));
        }
        if f.can_friend
            && let Some(buttons) = self.profile_friend_buttons(key, user_id, &p, cx)
        {
            column = column.child(motion::rise(
                under_box(&p).child(buttons),
                "profile-friends-in",
                Duration::from_millis(65),
                6.0,
            ));
        }
        if let (Some(sid), false) = (server, f.moderation.is_empty()) {
            let timed_out = f.member.as_ref().and_then(|m| timed_out_until(m, now_ms())).is_some();
            let mut row = under_box(&p).flex().gap(px(6.0));
            for permission in &f.moderation {
                let (glyph, label, action) = match permission {
                    P::TimeOutMembers => (
                        "hourglass",
                        if timed_out { t("workspace.popover.timedOut") } else { t("workspace.popover.timeout") },
                        Action::TimeOut(3_600),
                    ),
                    P::KickMembers => ("door-open", t("workspace.popover.kick"), Action::Kick),
                    _ => ("gavel", t("workspace.popover.ban"), Action::Ban(0)),
                };
                let (k, s, uid) = (key.to_owned(), sid.to_owned(), user_id.to_owned());
                let red = p.destructive;
                let soft = alpha(p.destructive, 0.1);
                row = row.child(
                    div()
                        .id(SharedString::from(format!("profile-mod-{label}")))
                        .flex_1()
                        .h(px(28.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .rounded(radius_xl())
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(soft).text_color(red))
                        .active(|s| s.top(px(1.0)))
                        .child(icon(glyph).size(px(14.0)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let dialog =
                                Dialog::Moderate { key: k.clone(), server: s.clone(), user_id: uid.clone(), action };
                            this.open_dialog(dialog, window, cx)
                        })),
                );
            }
            column = column.child(motion::rise(row, "profile-mod-in", Duration::from_millis(80), 6.0));
        }

        // Beside what opened it: on its side when there's room, else the other; the window's edges hold it in.
        let view = window.viewport_size();
        let (vw, vh) = (f32::from(view.width), f32::from(view.height));
        let (b, side) = self
            .people
            .anchor
            .unwrap_or((Bounds::new(window.mouse_position(), gpui_kit::size(px(1.0), px(1.0))), Side::Right));
        let (left, right, top) = (f32::from(b.left()), f32::from(b.right()), f32::from(b.top()));
        let beside_right = right + OFFSET;
        let beside_left = left - OFFSET - WIDTH;
        let x = match side {
            Side::Right if beside_right + WIDTH <= vw - EDGE => beside_right,
            Side::Right if beside_left >= EDGE => beside_left,
            Side::Left if beside_left >= EDGE => beside_left,
            Side::Left if beside_right + WIDTH <= vw - EDGE => beside_right,
            _ => beside_right,
        }
        .clamp(EDGE, (vw - EDGE - WIDTH).max(EDGE));
        let scroller = div().id("profile-scroll").max_h(px(vh - 2.0 * EDGE)).overflow_y_scroll().child(column);
        let popover = motion::rise(
            div().child(div().child(scroller).with_animation(
                SharedString::from(format!("profile-pop|{user_id}")),
                gpui_kit::Animation::new(Duration::from_millis(220)).with_easing(gpui_kit::ease_out_quint()),
                |el, t| el.opacity(t),
            )),
            SharedString::from(format!("profile-rise|{user_id}")),
            Duration::ZERO,
            6.0,
        );
        let mut layer = div().absolute().inset_0().child(
            gpui_kit::anchored()
                .position(point(px(x), px(top.max(EDGE))))
                .snap_to_window_with_margin(gpui_kit::Edges::all(px(EDGE)))
                .child(popover),
        );
        if let (Some(at), Some(sid)) = (self.people.roles_menu, server) {
            layer = layer.child(self.roles_menu(key, sid, user_id, &f, at, &p, cx));
        }
        layer.into_any_element()
    }

    /// The card itself (`ProfileCard`).
    #[allow(clippy::too_many_arguments)]
    fn card_body(
        &mut self,
        key: &str,
        user_id: &str,
        user: Option<&pb::User>,
        profile: Option<&pb::Profile>,
        f: &Facts,
        loading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = pal(cx);
        let now = now_ms();
        let name = f
            .member
            .as_ref()
            .map(|m| m.nickname.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| user.map(crate::core::store::user_name))
            .unwrap_or_else(|| t("common.someone"));
        let display = user.map(crate::core::store::user_name).unwrap_or_default();
        let status = user.and_then(|u| crate::ui::presence::custom_status(u, now));
        let accent = profile.and_then(|pr| pr.accent_color).unwrap_or(-1);
        let banner = profile.map(|pr| pr.banner_url.clone()).unwrap_or_default();
        let hidden = f.me && self.prefs.hides_personal();

        // The banner: their picture, or their color shaded toward the corner, or a sweep from their id.
        let banner = if accent < 0 && banner.is_empty() {
            // The web's `server-gradient` with its glow, which GPUI can't draw: painted once as an SVG.
            div().h(px(112.0)).w_full().child(gpui_kit::img(hue_banner(user_id, WIDTH - 2.0, 112.0)).size_full())
        } else {
            crate::ui::settings_account::banner_of(user_id, banner, accent, &p).h(px(112.0))
        };
        let banner = motion::once(
            banner,
            SharedString::from(format!("profile-banner|{user_id}|{accent}")),
            Duration::from_millis(600),
            |el, t| {
                let e = 1.0 - (1.0 - t).powi(3);
                el.opacity(e)
            },
        );

        // Their picture in a ring of the card's color, the dot on it, and their status beside it.
        let dot = f.presence.as_ref().map(|pr| {
            let status = crate::ui::presence::shown(pr.as_ref());
            div().absolute().right(px(2.0 - 5.0)).bottom(px(2.0 - 5.0)).child(crate::ui::presence::ringed_dot(
                status,
                20.0,
                5.0,
                p.card.into(),
                &p,
            ))
        });
        let picture = div()
            .flex_none()
            .size(px(86.0))
            .relative()
            .child(div().absolute().left(px(-1.0)).top(px(-1.0)).size(px(88.0)).rounded_full().bg(p.card))
            .child(
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(3.0))
                    .size(px(80.0))
                    .child(crate::ui::widgets::decorated(avatar(user, 80.0, &p), 80.0, f.decoration.as_deref()))
                    .children(dot),
            );
        let picture = motion::rise(
            div().child(picture),
            SharedString::from(format!("profile-face|{user_id}")),
            Duration::ZERO,
            6.0,
        );
        let top = div().mt(px(-44.0)).flex().items_end().gap(px(8.0)).child(picture).when_some(status, |el, status| {
            el.child(motion::rise(
                div()
                    .mb(px(36.0))
                    .min_w_0()
                    .rounded(radius_2xl())
                    .rounded_bl(radius_md())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_sm()
                    .line_height(px(20.0))
                    .shadow(shadow_md())
                    .child(div().line_clamp(2).child(status)),
                SharedString::from(format!("profile-status|{user_id}")),
                Duration::from_millis(150),
                4.0,
            ))
        });

        // Their name with a crown and an agent badge; under it their username, pronouns and display name.
        let pronouns = profile.map(|pr| pr.pronouns.clone()).filter(|x| !x.is_empty());
        let nickname = f.member.as_ref().map(|m| m.nickname.clone()).filter(|n| !n.is_empty() && *n != display);
        let username = user.map(|u| u.username.clone()).unwrap_or_default();
        let names = motion::rise(
            div()
                .mt(px(8.0))
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(px(6.0))
                        .text_xl()
                        .line_height(px(28.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(div().min_w_0().truncate().child(name))
                        .when(f.owner, |el| el.child(icon("crown").size(px(16.0)).text_color(rgb(0xfbbf24))))
                        .when(is_agent(user), |el| el.child(app_badge("profile-badge", "AGENT", &p))),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .min_w_0()
                        .items_center()
                        .gap_x(px(6.0))
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(div().truncate().child(if hidden {
                            "@••••••".to_owned()
                        } else {
                            format!("@{username}")
                        }))
                        .when_some(pronouns, |el, pronouns| {
                            el.child(
                                div()
                                    .rounded_full()
                                    .bg(p.muted)
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .text_xs()
                                    .line_height(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(alpha(p.foreground, 0.8))
                                    .child(pronouns),
                            )
                        })
                        .when_some(nickname.map(|_| display.clone()), |el, display| {
                            el.child(div().truncate().child(format!("· {display}")))
                        }),
                ),
            SharedString::from(format!("profile-names|{user_id}")),
            Duration::from_millis(60),
            8.0,
        );

        let mut body = div().relative().px(px(16.0)).pb(px(16.0)).child(top).child(names);
        if let Some(roles) = self.member_roles(key, user_id, f, &p, cx) {
            body = body.child(roles);
        }
        if let Some(Some(presence)) = &f.presence
            && !presence.activities.is_empty()
        {
            let leaving = self.profile_leaving.clone();
            body = body.children(
                crate::ui::presence::activity_cards(&presence.activities, leaving.as_deref(), now, &p, cx)
                    .into_iter()
                    .enumerate()
                    .map(|(n, card)| {
                        motion::rise(
                            div().child(card),
                            SharedString::from(format!("profile-activity|{user_id}|{n}")),
                            Duration::from_millis(50 + 40 * n as u64),
                            8.0,
                        )
                    }),
            );
            // A running timer counts: drawn again each second while the card is open.
            let timed = presence.activities.iter().any(|a| a.started_at.is_some() || a.ends_at.is_some());
            if timed && self.profile_tick.is_none() {
                self.profile_tick = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let _ = this.update(cx, |this, cx| {
                        this.profile_tick = None;
                        cx.notify();
                    });
                }));
            }
        }
        let bio = profile.map(|pr| pr.bio.clone()).filter(|b| !b.is_empty());
        if bio.is_some() || loading {
            let content = match bio {
                Some(bio) => div()
                    .max_h(px(192.0))
                    .text_sm()
                    .child(
                        crate::ui::text::markdown("profile-bio", crate::ui::text::images_as_links(&bio))
                            .selectable(true)
                            .w_full(),
                    )
                    .into_any_element(),
                None => {
                    let shimmer = |w: f32| div().h(px(12.0)).w(gpui_kit::relative(w)).rounded(px(4.0)).bg(p.muted);
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .py(px(4.0))
                        .child(shimmer(11.0 / 12.0))
                        .child(shimmer(2.0 / 3.0))
                        .into_any_element()
                }
            };
            body = body.child(motion::rise(
                div()
                    .mt(px(12.0))
                    .rounded(radius_2xl())
                    .bg(alpha(p.muted, 0.6))
                    .p(px(12.0))
                    .child(caps(&t("workspace.profile.aboutMe"), &p).mb(px(4.0)))
                    .child(content),
                SharedString::from(format!("profile-bio|{user_id}")),
                Duration::from_millis(100),
                8.0,
            ));
        }
        let since = crate::ui::text::ms_of(profile.and_then(|pr| pr.created_at.as_ref()));
        let joined = crate::ui::text::ms_of(f.member.as_ref().and_then(|m| m.joined_at.as_ref()));
        if since > 0 || joined > 0 {
            body = body.child(motion::rise(
                div()
                    .mt(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .when(since > 0, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(icon("calendar-heart").size(px(14.0)))
                                .child(t_with("workspace.profile.since", &[("date", Arg::Str(&day(since)))])),
                        )
                    })
                    .when(joined > 0, |el| {
                        el.child(
                            div()
                                .pl(px(20.0))
                                .child(t_with("workspace.profile.joined", &[("date", Arg::Str(&day(joined)))])),
                        )
                    }),
                SharedString::from(format!("profile-dates|{user_id}")),
                Duration::from_millis(140),
                6.0,
            ));
        }

        // Their effect plays over the card (others' only while you let them): in a server their
        // pick there, else their own, a built-in or one the instance or server offers.
        let own = profile.map(|pr| pr.effect.clone()).unwrap_or_default();
        let worn = (f.me || self.prefs.others_effects)
            .then(|| {
                self.core.shared.read(|s| {
                    s.instance(key).filter(|i| i.effects_on()).and_then(|i| {
                        let server = f.member.as_ref().map(|m| m.server_id.as_str()).filter(|s| !s.is_empty());
                        i.worn_effect(&own, server, f.member.as_ref())
                    })
                })
            })
            .flatten();
        let effect = worn.map(|spec| {
            let view = self.people.effect.clone();
            let seed = user_id.to_owned();
            view.update(cx, |v, cx| v.set(Some(&spec), &seed, accent, WIDTH, 420.0, true, cx));
            crate::ui::profile_effect::over_card(view)
        });
        let _ = window;
        div()
            .relative()
            .w(px(WIDTH))
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(crate::ui::settings_controls::shadow_xl())
            .child(banner)
            .child(body)
            .children(effect)
            .into_any_element()
    }

    /// Their roles as chips, highest first (`MemberRoles`): with Manage
    /// Roles, the ones below yours take off on hover, and a + adds more.
    fn member_roles(
        &mut self,
        key: &str,
        user_id: &str,
        f: &Facts,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        f.member.as_ref()?;
        if f.held.is_empty() && f.assignable.is_empty() {
            return None;
        }
        let server = f.member.as_ref().map(|m| m.server_id.clone()).unwrap_or_default();
        let busy = self.people.busy_role.clone();
        let mut chips = div().flex().flex_wrap().gap(px(4.0));
        for (role, removable) in &f.held {
            let dot = role_dot(role, p);
            let group = SharedString::from(format!("role-chip|{}", role.id));
            let lead = if *removable {
                let (k, s, uid, rid) = (key.to_owned(), server.clone(), user_id.to_owned(), role.id.clone());
                div()
                    .id(SharedString::from(format!("role-take|{}", role.id)))
                    .relative()
                    .size(px(12.0))
                    .flex_none()
                    .cursor_pointer()
                    .child(div().absolute().inset_0().group_hover(group.clone(), |s| s.opacity(0.0)).child(dot))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .opacity(0.0)
                            .text_color(p.destructive)
                            .group_hover(group.clone(), |s| s.opacity(1.0))
                            .child(icon("x").size(px(12.0))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.change_role(&k, &s, &uid, &rid, false, cx)
                    }))
                    .into_any_element()
            } else {
                dot.into_any_element()
            };
            let chip = div()
                .id(group.clone())
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
                .when(busy.as_deref() == Some(role.id.as_str()), |el| el.opacity(0.5))
                .child(lead)
                .child(div().truncate().child(role.name.clone()));
            chips = chips.child(motion::rise(
                chip,
                SharedString::from(format!("role-in|{user_id}|{}", role.id)),
                Duration::ZERO,
                2.0,
            ));
        }
        if !f.assignable.is_empty() {
            let open = self.people.roles_menu.is_some();
            let (muted, primary, ring) = (p.muted_foreground, p.primary, alpha(p.primary, 0.5));
            chips = chips.child(
                div()
                    .id("role-add")
                    .relative()
                    .size(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_dashed()
                    .border_color(if open { ring } else { p.border.into() })
                    .text_color(if open { primary } else { muted })
                    .cursor_pointer()
                    .hover(move |s| s.border_color(ring).text_color(primary))
                    // `data-[state=open]:rotate-45`: the plus turned into a cross.
                    .child(icon(if open { "x" } else { "plus" }).size(px(14.0)))
                    .child(
                        div().absolute().inset_0().child(
                            canvas(|bounds, _, _| ROLE_ADD.with(|c| *c.borrow_mut() = Some(bounds)), |_, _, _, _| {})
                                .size_full(),
                        ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.people.roles_menu = match this.people.roles_menu {
                            Some(_) => None,
                            // Where the web's menu settles under it.
                            None => {
                                ROLE_ADD.with(|c| *c.borrow()).map(|b| point(b.left() - px(7.0), b.bottom() + px(9.0)))
                            }
                        };
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .mt(px(12.0))
                .child(caps(&t("workspace.roles.heading"), p).mb(px(6.0)))
                .child(chips)
                .into_any_element(),
        )
    }

    /// The roles you may hand out, each checked when they have it (the + menu).
    #[allow(clippy::too_many_arguments)]
    fn roles_menu(
        &mut self,
        key: &str,
        server: &str,
        user_id: &str,
        f: &Facts,
        at: Point<Pixels>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let held: Vec<String> = f.member.as_ref().map(|m| m.role_ids.clone()).unwrap_or_default();
        let busy = self.people.busy_role.is_some();
        let mut list = div()
            .id("roles-menu")
            .w(px(224.0))
            .max_h(px(288.0))
            .overflow_y_scroll()
            .rounded(radius_md())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(4.0))
            .shadow(shadow_md())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.people.roles_menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_xs()
                    .line_height(px(16.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.muted_foreground)
                    .child(t("workspace.roles.assignable")),
            );
        for role in &f.assignable {
            let on = held.contains(&role.id);
            let (k, s, uid, rid) = (key.to_owned(), server.to_owned(), user_id.to_owned(), role.id.clone());
            let hover = p.accent;
            list = list.child(
                div()
                    .id(SharedString::from(format!("roles-menu|{}", role.id)))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(crate::ui::theme::radius_sm())
                    .text_sm()
                    .line_height(px(20.0))
                    .cursor_pointer()
                    .when(busy, |el| el.opacity(0.5))
                    .hover(move |s| s.bg(hover))
                    .child(role_dot(role, p))
                    .child(div().flex_1().truncate().child(role.name.clone()))
                    .when(on, |el| {
                        el.child(motion::rise(
                            div().text_color(p.primary).child(icon("check").size(px(16.0))),
                            SharedString::from(format!("roles-menu-on|{}", role.id)),
                            Duration::ZERO,
                            2.0,
                        ))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.change_role(&k, &s, &uid, &rid, !on, cx))),
            );
        }
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(at)
                .snap_to_window_with_margin(gpui_kit::Edges::all(px(8.0)))
                .child(motion::rise(list, "roles-menu-in", Duration::ZERO, -4.0)),
        )
        .with_priority(2)
        .into_any_element()
    }

    fn change_role(
        &mut self,
        key: &str,
        server: &str,
        user_id: &str,
        role_id: &str,
        give: bool,
        cx: &mut Context<Self>,
    ) {
        if self.people.busy_role.is_some() {
            return;
        }
        self.people.busy_role = Some(role_id.to_owned());
        let (core, k, s, uid, rid) =
            (self.core.clone(), key.to_owned(), server.to_owned(), user_id.to_owned(), role_id.to_owned());
        self.run(cx, async move { core.set_member_role(&k, &s, &uid, &rid, give).await }, |this, result, cx| {
            this.people.busy_role = None;
            if let Err(err) = result {
                this.toast("circle-alert", err.message, String::new(), None, None, cx);
            }
            cx.notify();
        });
        cx.notify();
    }
}

thread_local! {
    /// Where the card's + was drawn, for its menu.
    static ROLE_ADD: RefCell<Option<Bounds<Pixels>>> = const { RefCell::new(None) };
}

/// Someone's banner without a picture or color (the web's `.server-gradient`
/// on their hue: a 135° sweep under a soft glow at 30% 20%), as an SVG of
/// `w`×`h`; the card clips it to its corners. Made once per person and size.
fn hue_banner(user_id: &str, w: f32, h: f32) -> std::sync::Arc<gpui_kit::Image> {
    type Made = HashMap<String, std::sync::Arc<gpui_kit::Image>>;
    thread_local! {
        static MADE: RefCell<Made> = RefCell::new(HashMap::new());
    }
    let key = format!("{user_id}|{w}|{h}");
    if let Some(made) = MADE.with(|m| m.borrow().get(&key).cloned()) {
        return made;
    }
    let hue = crate::ui::widgets::hue_of(user_id);
    let hex = |deg: f32, s: f32, l: f32| {
        let c: gpui_kit::Rgba = gpui_kit::hsla((deg % 360.0) / 360.0, s, l, 1.0).into();
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
    };
    // CSS's 135° line runs through the middle, as long as the box's projection on it.
    let half = (w + h) * std::f32::consts::FRAC_1_SQRT_2 / 2.0;
    let d = half * std::f32::consts::FRAC_1_SQRT_2;
    let (cx, cy) = (w / 2.0, h / 2.0);
    // `circle at 30% 20%` reaches the farthest corner.
    let (gx, gy) = (0.3 * w, 0.2 * h);
    let reach = ((w - gx).powi(2) + (h - gy).powi(2)).sqrt();
    let glow = hex(hue, 0.9, 0.75);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {w} {h}">
<defs>
<linearGradient id="a" gradientUnits="userSpaceOnUse" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"><stop offset="0" stop-color="{from}"/><stop offset="1" stop-color="{to}"/></linearGradient>
<radialGradient id="b" gradientUnits="userSpaceOnUse" cx="{gx}" cy="{gy}" r="{reach}"><stop offset="0" stop-color="{glow}" stop-opacity="0.9"/><stop offset="0.6" stop-color="{glow}" stop-opacity="0"/></radialGradient>
</defs>
<rect width="{w}" height="{h}" fill="url(#a)"/>
<rect width="{w}" height="{h}" fill="url(#b)"/>
</svg>"#,
        W = w * 2.0,
        H = h * 2.0,
        x1 = cx - d,
        y1 = cy - d,
        x2 = cx + d,
        y2 = cy + d,
        from = hex(hue, 0.7, 0.55),
        to = hex(hue + 40.0, 0.7, 0.45),
    );
    let image = std::sync::Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Svg, svg.into_bytes()));
    MADE.with(|m| m.borrow_mut().insert(key, image.clone()));
    image
}

/// A role's color as a dot (`RoleDot`, size-3), grey for one without.
pub(crate) fn role_dot(role: &pb::Role, p: &Palette) -> gpui_kit::Div {
    let color: Hsla = role.color.map(|c| rgb(c as u32).into()).unwrap_or(alpha(p.muted_foreground, 0.5));
    div().flex_none().size(px(12.0)).rounded_full().bg(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_beside_the_opener_under_the_pointer() {
        let at = |x: f32, y: f32| point(px(x), px(y));
        let bounds = |x: f32, y: f32| Bounds::new(at(x, y), gpui_kit::size(px(100.0), px(20.0)));
        remember("u1", Side::Left, bounds(10.0, 10.0));
        remember("u1", Side::Right, bounds(10.0, 200.0));
        assert_eq!(anchor_for("u1", at(20.0, 15.0)), (bounds(10.0, 10.0), Side::Left));
        // Nothing under the pointer: the one drawn last.
        assert_eq!(anchor_for("u1", at(600.0, 600.0)), (bounds(10.0, 200.0), Side::Right));
        // Someone never drawn: the pointer.
        assert_eq!(anchor_for("nobody", at(5.0, 6.0)).0.origin, at(5.0, 6.0));
    }
}
