//! A server's member list, beside its channels. It's a view of its own,
//! drawn again only when its people change (not on every frame of the
//! window's animations), and only the rows in sight are built, so a server
//! with thousands of people scrolls like one with ten.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, EventEmitter, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb, uniform_list,
};

use crate::core::Core;
use crate::core::i18n::t;
use crate::core::store::user_name;
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{alpha, corner};
use crate::ui::widgets::{app_badge, avatar, decorated, icon, is_agent, pal};

/// Rows are this tall, every one, which is what lets the list skip the rest.
const ROW: f32 = 44.0;
/// Rows rise in like this while the list is new, not as you scroll to them.
const ENTERING: Duration = Duration::from_millis(900);

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
    /// Their dot; None on an instance without presence.
    status: Option<pb::PresenceStatus>,
    /// What they're doing, for under their name.
    activity: Option<String>,
    /// The decoration around their avatar, its picture's link.
    decoration: Option<SharedString>,
}

pub struct MembersView {
    core: Arc<Core>,
    pub key: String,
    pub server: String,
    rows: Rc<Vec<Item>>,
    digest: u64,
    born: Instant,
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
        let mut this = Self { core, key, server, rows: Rc::default(), digest: 0, born: Instant::now() };
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
        for item in &rows {
            match item {
                Item::Heading(name, color, n) => (name, color, n).hash(&mut h),
                Item::Member(r) => {
                    (&r.user.id, &r.user.avatar_url, &r.name, r.color, r.agent, r.timed_out, r.mine).hash(&mut h);
                    (r.status.map(|s| s as i32), &r.activity, &r.decoration).hash(&mut h);
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
            self.rows = Rc::new(rows);
            cx.notify();
        }
    }
}

/// The list's lines, from what's known of a server's people: everyone online
/// under their highest role that's shown apart, then everyone else online,
/// then everyone offline (where the instance says who is). Also when the
/// soonest time-out ends.
fn lines(i: &crate::core::store::InstanceState, server: &str, now: i64) -> (Vec<Item>, Option<i64>) {
    let mut ends: Option<i64> = None;
    let me = i.me.as_ref().map(|m| m.id.as_str()).unwrap_or_default();
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
            activity: presence.and_then(|p| p.activities.first()).map(crate::ui::presence::line),
            decoration: i.decoration_url(Some(server), Some(m), Some(&user)).map(|u| SharedString::from(u.to_owned())),
            color,
            agent: is_agent(Some(&user)),
            timed_out: until.is_some(),
            mine: user.id == me,
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
        let entering = self.born.elapsed() < ENTERING;
        let count = rows.len();
        let list = uniform_list(
            "member-rows",
            count,
            cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                let p = pal(cx);
                let now_rows = rows.clone();
                range
                    .filter_map(|n| now_rows.get(n).map(|row| (n, row)))
                    .map(|(n, item)| match item {
                        Item::Heading(name, color, members) => heading(name, *color, *members, n, &p),
                        Item::Member(row) => member_row(row, n, entering, &p, cx).into_any_element(),
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .px(px(8.0))
        .pt(px(4.0))
        .pb(px(12.0));
        div().size_full().flex().flex_col().bg(p.side_surface).border_l_1().border_color(p.border).child(list)
    }
}

/// A group's name over its people, as tall as a row so the list can skip what's out of sight.
fn heading(
    name: &str,
    color: Option<u32>,
    members: usize,
    n: usize,
    p: &crate::ui::theme::Palette,
) -> gpui_kit::AnyElement {
    div()
        .id(SharedString::from(format!("member-heading|{name}|{n}")))
        .h(px(ROW))
        .px(px(8.0))
        .pb(px(6.0))
        .flex()
        .items_end()
        .gap(px(6.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(p.muted_foreground)
        .when_some(color, |el, c| el.child(div().mb(px(3.0)).size(px(7.0)).rounded_full().bg(rgb(c))))
        .child(format!("{} — {members}", name.to_uppercase()))
        .into_any_element()
}

fn member_row(
    row: &Row,
    n: usize,
    entering: bool,
    p: &crate::ui::theme::Palette,
    cx: &mut Context<MembersView>,
) -> impl IntoElement {
    let user = &row.user;
    let hover = alpha(p.primary, 0.08);
    let amber = gpui_kit::hsla(0.11, 0.9, if p.dark { 0.62 } else { 0.42 }, 1.0);
    let color: Hsla = row.color.map(|c| rgb(c).into()).unwrap_or(p.foreground.into());
    let uid = user.id.clone();
    let el = div()
        .id(SharedString::from(format!("member|{}", user.id)))
        .group("member")
        .h(px(ROW))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .rounded(corner(12.0))
        .hover(move |s| s.bg(hover))
        .cursor_pointer()
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(MembersEvent::Open { user_id: uid.clone() })))
        .on_mouse_down(gpui_kit::MouseButton::Right, {
            let uid = user.id.clone();
            cx.listener(move |_, ev: &gpui_kit::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                cx.emit(MembersEvent::Menu { user_id: uid.clone(), at: ev.position });
            })
        })
        .on_hover({
            let uid = user.id.clone();
            cx.listener(move |_, on: &bool, _, cx| cx.emit(MembersEvent::Hover { user_id: uid.clone(), on: *on }))
        })
        // Someone offline is faded, until pointed at.
        .when(row.status == Some(pb::PresenceStatus::Offline), |el| {
            el.opacity(0.45).hover(move |s| s.bg(hover).opacity(1.0))
        })
        .child(
            div()
                .relative()
                .flex_none()
                .child(decorated(avatar(Some(user), 32.0, p), 32.0, row.decoration.as_deref()))
                .when_some(row.status.filter(|s| *s != pb::PresenceStatus::Offline), |el, status| {
                    el.child(crate::ui::presence::avatar_dot(status, 14.0, opaque(p.side_surface), p))
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
                        .gap(px(6.0))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_weight(FontWeight::BOLD)
                                .text_color(color)
                                .child(row.name.clone()),
                        )
                        .when(row.agent, |el| {
                            el.child(app_badge(SharedString::from(format!("member-badge|{}", user.id)), "AGENT", p))
                        }),
                )
                .when_some(row.activity.clone(), |el, line| {
                    el.child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(line),
                    )
                }),
        )
        .when(row.timed_out, |el| el.child(icon("hourglass").size(px(14.0)).text_color(amber)))
        .when(!row.mine, |el| {
            el.child(
                div()
                    .opacity(0.0)
                    .group_hover("member", |s| s.opacity(1.0))
                    .text_color(p.primary)
                    .child(icon("chevron-right").size(px(14.0))),
            )
        });
    if entering && n < 24 {
        motion::rise(
            el,
            SharedString::from(format!("member-in|{}", user.id)),
            Duration::from_millis((14 * n) as u64),
            6.0,
        )
        .into_any_element()
    } else {
        el.into_any_element()
    }
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
