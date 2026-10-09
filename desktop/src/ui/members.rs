//! A server's member list, beside its channels. It's a view of its own,
//! drawn again only when its people change (not on every frame of the
//! window's animations), and only the rows in sight are built, so a server
//! with thousands of people scrolls like one with ten.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnimationExt as _, Context, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::Core;
use crate::core::i18n::t;
use crate::core::store::user_name;
use crate::pb;
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::alpha;
use crate::ui::widgets::{app_badge, avatar, icon, is_agent, pal};

/// The web's member line: `row-y` (6px each side) around a 14px name over a 16px subtitle.
const ROW: f32 = 48.0;
/// A section's heading is a 16px line with 4px under it, and 16px over it after the first.
const HEADING: f32 = 20.0;
const SECTION_GAP: f32 = 16.0;
/// How long someone who just joined is still sliding in.
const ENTERING: Duration = Duration::from_millis(900);
/// Lines glide to a new place like this (the web's `.member-line`: 0.4s, ease-out quint).
const GLIDE: Duration = Duration::from_millis(400);
/// The list's padding above its first line and below its last.
const PAD: f32 = 16.0;

pub enum MembersEvent {
    Open {
        user_id: String,
    },
    /// Right-clicked: their menu, at the pointer.
    Menu {
        user_id: String,
        at: gpui_kit::Point<gpui_kit::Pixels>,
    },
    /// The pointer's on them, or left, for Shift+F10.
    Hover {
        user_id: String,
        on: bool,
    },
}

/// A line in the list: a group's heading, or someone in it.
enum Item {
    /// A role shown apart (or "Members" for everyone else), its color and how many are in it.
    Heading(String, Option<u32>, usize),
    Member(Box<Row>),
}

struct Row {
    user: pb::User,
    name: String,
    color: Option<u32>,
    agent: bool,
    timed_out: bool,
    mine: bool,
    /// Owns the server: a crown by their name.
    owner: bool,
    /// Their custom status, for under their name when they're doing nothing.
    status_text: Option<String>,
    /// The decoration around their avatar, its picture's link (docs/profile-items.md).
    decoration: Option<SharedString>,
    /// Their dot; None on an instance without presence.
    status: Option<pb::PresenceStatus>,
    /// What they're doing, for under their name.
    /// The line and the activity's name in it (boxed: rows stay small).
    activity: Option<Box<(String, String)>>,
}

pub struct MembersView {
    core: Arc<Core>,
    pub key: String,
    pub server: String,
    rows: Rc<Vec<Item>>,
    digest: u64,
    /// Where each line's top was last time, by its key, and each line's in order.
    tops: HashMap<String, f32>,
    order: Vec<f32>,
    /// Lines on their way to a new place: how far off it they started, and when.
    moving: Rc<HashMap<String, (f32, Instant)>>,
    /// Everyone in the server last time, and who wasn't, since when: they slide in.
    known: std::collections::HashSet<String>,
    joined: Option<Rc<(std::collections::HashSet<String>, Instant)>>,
    /// Lines of two heights (headings and people), drawn only while in sight.
    list: gpui_kit::ListState,
}

impl EventEmitter<MembersEvent> for MembersView {}

impl MembersView {
    pub fn new(core: Arc<Core>, key: String, server: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut changes = core.changes();
        cx.spawn_in(window, async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let list = gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, px(240.0));
        let mut this = Self {
            core,
            key,
            server,
            rows: Rc::default(),
            digest: 0,
            tops: HashMap::new(),
            order: Vec::new(),
            moving: Rc::default(),
            known: Default::default(),
            joined: None,
            list,
        };
        this.refresh(cx);
        this
    }

    /// Reads the people again, and draws again only if something shown changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = crate::core::dms::now_ms();
        let (rows, ends) = self.core.shared.read(|s| match s.instance(&self.key) {
            Some(i) => lines(i, &self.server, now),
            None => (Vec::new(), None),
        });
        let mut h = DefaultHasher::new();
        (crate::ui::theme::role_names(), crate::ui::theme::role_beside()).hash(&mut h);
        for item in &rows {
            match item {
                Item::Heading(name, color, n) => (name, color, n).hash(&mut h),
                Item::Member(r) => {
                    (&r.user.id, &r.user.avatar_url, &r.name, r.color, r.agent, r.timed_out, r.mine).hash(&mut h);
                    (r.owner, &r.status_text, &r.user.username, &r.decoration).hash(&mut h);
                    (r.status.map(|s| s as i32), &r.activity).hash(&mut h);
                }
            }
        }
        let digest = h.finish();
        // A time-out ending is news too, though nothing else changes.
        if let Some(ends) = ends {
            let wait = Duration::from_millis((ends - now).max(0) as u64 + 50);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| this.refresh(cx));
            })
            .detach();
        }
        if digest != self.digest {
            self.digest = digest;
            // Only people who just joined slide in, not the first ones seen.
            let ids: std::collections::HashSet<String> = rows
                .iter()
                .filter_map(|item| match item {
                    Item::Member(r) => Some(r.user.id.clone()),
                    Item::Heading(..) => None,
                })
                .collect();
            if !self.known.is_empty() {
                let fresh: std::collections::HashSet<String> = ids.difference(&self.known).cloned().collect();
                if !fresh.is_empty() {
                    self.joined = Some(Rc::new((fresh, Instant::now())));
                }
            }
            self.known = ids;
            self.place(&rows, cx.reduce_motion());
            self.rows = Rc::new(rows);
            cx.notify();
        }
    }
}

impl MembersView {
    /// Works out where each line now sits, keeps the list where it was scrolled
    /// to, and sets the lines that were in sight gliding from where they were
    /// to where they are (the web's `transform` transition). Lines that weren't
    /// in sight just appear, as they do on the web.
    fn place(&mut self, rows: &[Item], still: bool) {
        let mut top = 0.0;
        let mut order = Vec::with_capacity(rows.len());
        let mut tops = HashMap::with_capacity(rows.len());
        for (n, item) in rows.iter().enumerate() {
            order.push(top);
            tops.insert(key(item), top);
            top += height(item, n, rows.len());
        }
        // Where the list was scrolled to, in pixels, and how much of it showed.
        let at = self.list.logical_scroll_top();
        let scrolled = self.order.get(at.item_ix).copied().unwrap_or(0.0) + f32::from(at.offset_in_item);
        let seen = f32::from(self.list.viewport_bounds().size.height).max(ROW);
        let mut moving = HashMap::new();
        if !still {
            let now = Instant::now();
            for (k, &to) in &tops {
                let Some(&was) = self.tops.get(k) else { continue };
                // Still on its way somewhere: it carries on from where it is.
                let off = self.moving.get(k).map_or(0.0, |&(from, at)| glide_offset(from, at));
                let from = was + off - to;
                let in_sight = was + off + ROW >= scrolled && was + off <= scrolled + seen;
                if from.abs() > 0.5 && in_sight {
                    moving.insert(k.clone(), (from, now));
                }
            }
        }
        self.list.reset(rows.len());
        // Back to where it was scrolled to (resetting goes to the top).
        if scrolled > 0.0 {
            let ix = order.partition_point(|&t| t <= scrolled).saturating_sub(1);
            let offset = scrolled - order.get(ix).copied().unwrap_or(0.0);
            self.list.scroll_to(gpui_kit::ListOffset { item_ix: ix, offset_in_item: px(offset) });
        }
        self.tops = tops;
        self.order = order;
        self.moving = Rc::new(moving);
    }
}

/// A line's key, the same wherever it moves: a person's id, or a heading's name.
fn key(item: &Item) -> String {
    match item {
        Item::Heading(name, ..) => format!("h|{name}"),
        Item::Member(r) => format!("u|{}", r.user.id),
    }
}

/// How tall line `n` of `count` is, with the list's padding on the first and last.
fn height(item: &Item, n: usize, count: usize) -> f32 {
    let own = match item {
        Item::Heading(..) if n == 0 => HEADING,
        Item::Heading(..) => HEADING + SECTION_GAP,
        Item::Member(_) => ROW,
    };
    own + if n == 0 { PAD } else { 0.0 } + if n + 1 == count { PAD } else { 0.0 }
}

/// How far off its place a line that started `from` away at `at` is now.
fn glide_offset(from: f32, at: Instant) -> f32 {
    let t = (at.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.0);
    from * (1.0 - gpui_kit::ease_out_quint()(t))
}

/// The list's lines, from what's known of a server's people: everyone online
/// under their highest role that's shown apart, then everyone else online,
/// then everyone offline (where the instance says who is). Also when the
/// soonest time-out ends.
fn lines(i: &crate::core::store::InstanceState, server: &str, now: i64) -> (Vec<Item>, Option<i64>) {
    let mut ends: Option<i64> = None;
    let me = i.me.as_ref().map(|m| m.id.as_str()).unwrap_or_default();
    let owner = i.server(server).map(|s| s.owner_id.clone()).unwrap_or_default();
    let roles = i.roles.get(server);
    let Some(members) = i.members.get(server) else { return (Vec::new(), None) };
    let people = i.people.as_ref();
    let empty = Vec::new();
    let ranked = roles.unwrap_or(&empty);
    let mut groups: Vec<(&pb::Role, Vec<Row>)> =
        ranked.iter().filter(|r| r.hoist && r.id != server).map(|r| (r, Vec::new())).collect();
    let mut rest: Vec<Row> = Vec::new();
    let mut offline: Vec<Row> = Vec::new();
    let rows = members.iter().filter(|m| !m.pending).filter_map(|m| {
        let user = m.user.clone()?;
        // The first of their roles (by rank) that has a colour.
        let color = roles.and_then(|roles| {
            roles.iter().filter(|r| m.role_ids.contains(&r.id)).find_map(|r| r.color).map(|c| c as u32)
        });
        let until = crate::core::moderation::timed_out_until(m, now);
        if let Some(until) = until {
            ends = Some(ends.map_or(until, |e| e.min(until)));
        }
        let presence = people.and_then(|people| people.get(&user.id));
        Some(Row {
            name: if m.nickname.is_empty() { user_name(&user) } else { m.nickname.clone() },
            status: people.map(|_| crate::ui::presence::shown(presence)),
            activity: presence
                .and_then(|p| p.activities.first())
                .map(|a| Box::new((crate::ui::presence::line(a), a.name.clone()))),
            color,
            agent: is_agent(Some(&user)),
            timed_out: until.is_some(),
            mine: user.id == me,
            owner: !owner.is_empty() && user.id == owner,
            status_text: crate::ui::presence::custom_status(&user, now),
            decoration: i.decoration_url(Some(server), Some(m), Some(&user)).map(|u| SharedString::from(u.to_owned())),
            user,
        })
        .map(|row| (m, row))
    });
    for (m, row) in rows {
        if row.status == Some(pb::PresenceStatus::Offline) {
            offline.push(row);
            continue;
        }
        match groups.iter_mut().find(|(r, _)| m.role_ids.contains(&r.id)) {
            Some((_, list)) => list.push(row),
            None => rest.push(row),
        }
    }
    let mut out = Vec::new();
    for (role, list) in groups.into_iter().filter(|(_, l)| !l.is_empty()) {
        out.push(Item::Heading(role.name.clone(), role.color.map(|c| c as u32), list.len()));
        out.extend(list.into_iter().map(|row| Item::Member(Box::new(row))));
    }
    if !rest.is_empty() {
        let label = if people.is_some() { "chat.members.online" } else { "chat.members.members" };
        out.push(Item::Heading(t(label), None, rest.len()));
        out.extend(rest.into_iter().map(|row| Item::Member(Box::new(row))));
    }
    if !offline.is_empty() {
        out.push(Item::Heading(t("chat.members.offline"), None, offline.len()));
        out.extend(offline.into_iter().map(|row| Item::Member(Box::new(row))));
    }
    (out, ends)
}

impl Render for MembersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let rows = self.rows.clone();
        let moving = self.moving.clone();
        let this = cx.entity().downgrade();
        // The web's list: 16px above and below, 8px at the sides.
        let joined = self.joined.clone();
        let list = gpui_kit::list(self.list.clone(), move |n, window, cx| {
            let p = pal(cx);
            let first = n == 0;
            let last = n + 1 == rows.len();
            let el = match rows.get(n) {
                Some(Item::Heading(name, color, members)) => heading(name, *color, *members, n, first, &p, window, cx),
                Some(Item::Member(row)) => {
                    let fresh = joined.as_ref().is_some_and(|j| j.1.elapsed() < ENTERING && j.0.contains(&row.user.id));
                    member_row(row, fresh, &p, this.clone(), window, cx)
                }
                None => div().into_any_element(),
            };
            let line = div().px(px(8.0)).when(first, |el| el.pt(px(PAD))).when(last, |el| el.pb(px(PAD))).child(el);
            // A line that moved glides from where it was.
            let glide = rows.get(n).map(key).and_then(|k| moving.get(&k).map(|&m| (k, m)));
            match glide {
                Some((k, (from, at))) if at.elapsed() < GLIDE => line
                    .with_animation(
                        SharedString::from(format!("member-glide|{k}|{at:?}")),
                        gpui_kit::Animation::new(GLIDE),
                        move |el, _| el.translate_y(px(glide_offset(from, at))),
                    )
                    .into_any_element(),
                _ => line.into_any_element(),
            }
        })
        .flex_1();
        div().size_full().flex().flex_col().bg(p.side_surface).border_l_1().border_color(p.border).child(list)
    }
}

/// A group's name over its people: the web's 12px bold capitals, with the role's dot.
#[allow(clippy::too_many_arguments)]
fn heading(
    name: &str,
    color: Option<u32>,
    members: usize,
    n: usize,
    first: bool,
    p: &crate::ui::theme::Palette,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let text = crate::core::i18n::t_with(
        "chat.members.heading",
        &[
            ("name", crate::core::i18n::Arg::Str(&name.to_uppercase())),
            ("count", crate::core::i18n::Arg::Num(members as i64)),
        ],
    );
    // The count rolls as people come and go (the web's `Count`).
    let label = match around_count(&text, members) {
        Some((before, after)) => div()
            .flex()
            .min_w_0()
            .child(tracked(before, WIDE))
            .child(motion::count(format!("member-count|{name}"), members as u64, None, 12.0, window, cx))
            .child(tracked(after, WIDE))
            .into_any_element(),
        None => tracked(text, WIDE).into_any_element(),
    };
    div()
        .id(SharedString::from(format!("member-heading|{name}|{n}")))
        .when(!first, |el| el.pt(px(SECTION_GAP)))
        .child(
            div()
                .h(px(HEADING))
                .pb(px(4.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .when_some(color, |el, c| el.child(div().flex_none().size(px(8.0)).rounded_full().bg(rgb(c))))
                .child(label),
        )
        .into_any_element()
}

/// `text` cut around where `count` is written (the last time), so the number
/// can roll on its own: None when it isn't there as plain digits.
pub(crate) fn around_count(text: &str, count: usize) -> Option<(String, String)> {
    let digits = count.to_string();
    let at = text.rfind(&digits)?;
    Some((text[..at].to_owned(), text[at + digits.len()..].to_owned()))
}

#[allow(clippy::too_many_arguments)]
fn member_row(
    row: &Row,
    fresh: bool,
    p: &crate::ui::theme::Palette,
    this: gpui_kit::WeakEntity<MembersView>,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let user = &row.user;
    let hover = alpha(p.muted, 0.7);
    let amber = gpui_kit::hsla(0.11, 0.9, if p.dark { 0.62 } else { 0.42 }, 1.0);
    // The web's `RoleName`: the role's color on the name, or as a dot beside it, as the Role colors setting says.
    let color: Hsla = row
        .color
        .filter(|_| crate::ui::theme::role_names())
        .map(|c| rgb(c).into())
        .unwrap_or(crate::ui::widgets::name_tint(&row.user.id, p));
    let beside = row.color.filter(|_| crate::ui::theme::role_beside());
    let uid = user.id.clone();
    let offline = row.status == Some(pb::PresenceStatus::Offline);
    let el = div()
        .id(SharedString::from(format!("member|{}", user.id)))
        .group("member")
        .h(px(ROW))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .rounded(crate::ui::theme::radius_lg())
        // Someone offline is faded, until pointed at (one hover style: GPUI takes only one).
        .hover(move |s| if offline { s.bg(hover).opacity(1.0) } else { s.bg(hover) })
        .when(offline, |el| el.opacity(0.45))
        .cursor_pointer()
        .on_click({
            let this = this.clone();
            move |_, _, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(MembersEvent::Open { user_id: uid.clone() }));
            }
        })
        .on_mouse_down(gpui_kit::MouseButton::Right, {
            let (uid, this) = (user.id.clone(), this.clone());
            move |ev: &gpui_kit::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let _ = this.update(cx, |_, cx| cx.emit(MembersEvent::Menu { user_id: uid.clone(), at: ev.position }));
            }
        })
        .on_hover({
            let (uid, this) = (user.id.clone(), this.clone());
            move |on: &bool, _, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(MembersEvent::Hover { user_id: uid.clone(), on: *on }));
            }
        })
        // The card opens beside the button inside the row (the web's `px-2`), 10px off.
        .child(
            div()
                .absolute()
                .left(px(8.0))
                .right(px(8.0))
                .top(px(6.0))
                .bottom(px(6.0))
                .child(crate::ui::profile_card::mark(&user.id, crate::ui::profile_card::Side::Left)),
        )
        .child(
            // The picture swells a little while the row's pointed at.
            div()
                .id("member-face")
                .relative()
                .flex_none()
                .mr(px(6.0))
                .group_hover("member", |s| s.scale(1.05))
                .group_active("member", |s| s.scale(0.95))
                .child(crate::ui::widgets::decorated(avatar(Some(user), 32.0, p), 32.0, row.decoration.as_deref()))
                .when_some(row.status.filter(|s| *s != pb::PresenceStatus::Offline), |el, status| {
                    el.child(crate::ui::presence::avatar_dot(status, 11.2, 3.0, opaque(p.side_surface).into(), p))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_weight(FontWeight::BOLD)
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .text_color(color)
                                .child(row.name.clone()),
                        )
                        .when_some(beside, |el, c| {
                            el.child(
                                div()
                                    .flex_none()
                                    .size(px(12.0))
                                    .rounded_full()
                                    .border_2()
                                    .border_color(p.background)
                                    .bg(rgb(c)),
                            )
                        })
                        .when(row.owner, |el| {
                            el.child(icon("crown").size(px(12.0)).text_color(gpui_kit::rgb(0xfbbf24)))
                        })
                        .when(row.timed_out, |el| {
                            el.child(motion::pop(
                                div().flex_none().child(icon("hourglass").size(px(12.0)).text_color(amber)),
                                "member-timed-out",
                                0.0,
                                -90.0,
                                Duration::ZERO,
                            ))
                        })
                        .when(row.agent, |el| {
                            el.child(app_badge(SharedString::from(format!("member-badge|{}", user.id)), "AGENT", p))
                        }),
                )
                // Under the name: what they're doing, else their status, else their username.
                .child(subtitle(
                    &user.id,
                    &row.activity,
                    &row.status_text,
                    window,
                    cx,
                    div()
                        .h(px(16.0))
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(match (&row.activity, &row.status_text) {
                            (Some(line), _) => crate::ui::presence::rich_line(&line.0, &line.1, p),
                            (None, Some(status)) => gpui_kit::StyledText::new(status.clone()),
                            (None, None) => gpui_kit::StyledText::new(format!("@{}", row.user.username)),
                        }),
                )),
        );
    if fresh {
        // Someone who just joined slides in from the side.
        motion::slide_in(div().child(el), SharedString::from(format!("member-joined|{}", user.id)), 16.0)
            .into_any_element()
    } else {
        el.into_any_element()
    }
}

/// The line under a name, rising into place when what it says changes (the
/// web's `MemberSubtitle`); nothing moves when it first shows.
fn subtitle(
    user_id: &str,
    activity: &Option<Box<(String, String)>>,
    status: &Option<String>,
    window: &mut Window,
    cx: &mut gpui_kit::App,
    line: gpui_kit::Div,
) -> gpui_kit::AnyElement {
    let mut h = DefaultHasher::new();
    (activity, status).hash(&mut h);
    let said = h.finish();
    let state = window.use_keyed_state(SharedString::from(format!("member-sub|{user_id}")), cx, |_, _| (said, 0u64));
    let changes = state.update(cx, |(was, changes), _| {
        if *was != said {
            *was = said;
            *changes += 1;
        }
        *changes
    });
    if changes == 0 {
        return line.into_any_element();
    }
    crate::ui::motion::spring_in(
        line,
        SharedString::from(format!("member-sub-in|{user_id}|{changes}")),
        (500.0, 32.0),
        Duration::ZERO,
        |el, t| el.opacity(t.clamp(0.0, 1.0)).translate_y(px((1.0 - t) * 12.0)),
    )
}

/// The list's color without see-through, for the ring that cuts a dot out of a picture.
fn opaque(color: Hsla) -> gpui_kit::Rgba {
    gpui_kit::Rgba { a: 1.0, ..color.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn member(id: &str, roles: &[&str]) -> pb::Member {
        pb::Member {
            user: Some(pb::User { id: id.into(), username: id.into(), ..Default::default() }),
            role_ids: roles.iter().map(|r| r.to_string()).collect(),
            ..Default::default()
        }
    }

    fn here(id: &str, status: pb::PresenceStatus) -> (String, pb::Presence) {
        (id.into(), pb::Presence { user_id: id.into(), status: status as i32, ..Default::default() })
    }

    /// Headings and names in order, as the list shows them.
    fn shown(i: &crate::core::store::InstanceState) -> Vec<String> {
        lines(i, "s", 0)
            .0
            .iter()
            .map(|item| match item {
                Item::Heading(name, _, n) => format!("{name} {n}"),
                Item::Member(row) => row.name.clone(),
            })
            .collect()
    }

    #[test]
    fn offline_people_come_last_under_their_own_heading() {
        let mut i = crate::core::store::InstanceState::new("k", "https://k");
        i.roles.insert(
            "s".into(),
            vec![pb::Role { id: "mods".into(), name: "Mods".into(), hoist: true, ..Default::default() }],
        );
        i.members.insert(
            "s".into(),
            vec![member("ana", &["mods"]), member("bo", &["mods"]), member("cy", &[]), member("di", &[])],
        );
        // Before the instance says who's online, everyone's together as before.
        assert_eq!(shown(&i), ["Mods 2", "ana", "bo", "Members 2", "cy", "di"]);
        // Bo is offline and Di is invisible, which reaches us as not online at all.
        i.people =
            Some(HashMap::from([here("ana", pb::PresenceStatus::Idle), here("cy", pb::PresenceStatus::DoNotDisturb)]));
        assert_eq!(shown(&i), ["Mods 1", "ana", "Online 1", "cy", "Offline 2", "bo", "di"]);
    }
}
