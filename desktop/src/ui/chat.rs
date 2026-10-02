//! The middle of the window: a channel or a private conversation (its
//! messages and the composer), the member list, an instance's page, or home.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::Textarea;
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Context, Focusable as _, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
    div, px, rgb,
};

use crate::core::config::Density;
use crate::core::store::{Connection, user_name};
use crate::core::vault::ItemKind;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Nav, Target};
use crate::ui::motion;
use crate::ui::text::{clock, images_as_links, ms_of, when};
use crate::ui::theme::{Palette, alpha, mix};
use crate::ui::widgets::{
    avatar, card, conn_dot, error_line, fuwa_mark, icon, icon_button, pal, primary_button, soft_button,
};

/// Messages from the same person this close together sit under one header.
const GROUP_MS: i64 = 7 * 60 * 1000;

#[derive(Clone)]
pub enum Row {
    /// More history above: a button that loads it.
    Older {
        loading: bool,
    },
    /// The top of a channel or a conversation.
    Start {
        icon: &'static str,
        title: String,
        body: String,
    },
    /// Something that happened, not something said.
    Note {
        id: String,
        icon: &'static str,
        text: String,
    },
    Msg(Box<Msg>),
}

#[derive(Clone)]
pub struct Msg {
    /// The message's id, a private message's sequence, or `p{nonce}` while sending.
    pub id: String,
    pub user: Option<pb::User>,
    pub name: String,
    pub color: Option<Hsla>,
    pub content: String,
    pub at: i64,
    pub edited: bool,
    pub head: bool,
    pub mine: bool,
    pub pending: bool,
    pub failed: Option<String>,
    pub nonce: u64,
    pub unreadable: bool,
}

impl Row {
    fn id(&self) -> String {
        match self {
            Row::Older { .. } => "older".into(),
            Row::Start { .. } => "start".into(),
            Row::Note { id, .. } => id.clone(),
            Row::Msg(m) => m.id.clone(),
        }
    }

    fn digest(&self, h: &mut DefaultHasher) {
        match self {
            Row::Older { loading } => ("older", loading).hash(h),
            Row::Start { title, .. } => ("start", title).hash(h),
            Row::Note { id, text, .. } => (id, text).hash(h),
            Row::Msg(m) => (&m.id, &m.content, m.edited, m.head, m.pending, &m.failed, &m.name).hash(h),
        }
    }
}

impl FuwaApp {
    /// What the open list shows, row by row.
    fn rows(&self) -> Vec<Row> {
        match self.target() {
            Some(Target::Channel { key, server, channel }) => self.core.shared.read(|s| {
                let Some(i) = s.instance(&key) else { return Vec::new() };
                let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                let mut rows = Vec::new();
                let Some(loaded) = i.messages.get(&channel) else { return rows };
                let name = i.channel(&server, &channel).map(|c| c.name.clone()).unwrap_or_default();
                if loaded.has_more {
                    rows.push(Row::Older { loading: loaded.loading });
                } else {
                    rows.push(Row::Start {
                        icon: "hash",
                        title: format!("Welcome to #{name}"),
                        body: format!("This is the start of #{name}."),
                    });
                }
                for m in &loaded.items {
                    if m.kind == pb::MessageKind::MemberJoined as i32 {
                        rows.push(Row::Note {
                            id: m.id.clone(),
                            icon: "sparkles",
                            text: format!("{} joined the server. Say hi!", i.display_name(Some(&server), &m.author_id)),
                        });
                        continue;
                    }
                    rows.push(Row::Msg(Box::new(Msg {
                        id: m.id.clone(),
                        user: i.users.get(&m.author_id).cloned(),
                        name: i.display_name(Some(&server), &m.author_id),
                        color: i.name_color(&server, &m.author_id).map(|c| rgb(c).into()),
                        content: m.content.clone(),
                        at: ms_of(m.created_at.as_ref()),
                        edited: m.edited_at.is_some(),
                        head: true,
                        mine: m.author_id == me,
                        pending: false,
                        failed: None,
                        nonce: 0,
                        unreadable: false,
                    })));
                }
                for p in i.pending.get(&channel).into_iter().flatten() {
                    rows.push(Row::Msg(Box::new(Msg {
                        id: format!("p{}", p.nonce),
                        user: i.me.clone(),
                        name: i.display_name(Some(&server), &me),
                        color: i.name_color(&server, &me).map(|c| rgb(c).into()),
                        content: p.content.clone(),
                        at: p.created_at_ms,
                        edited: false,
                        head: true,
                        mine: true,
                        pending: true,
                        failed: p.failed.clone(),
                        nonce: p.nonce,
                        unreadable: false,
                    })));
                }
                group(&mut rows);
                rows
            }),
            Some(Target::Dm { key, conversation }) => self.core.shared.read(|s| {
                let Some(i) = s.instance(&key) else { return Vec::new() };
                let me = i.me.clone();
                let me_id = me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                let users: Vec<pb::User> =
                    i.dms.conversations.iter().find(|c| c.id == conversation).map(|c| c.users.clone()).unwrap_or_default();
                let person = |id: &str| users.iter().find(|u| u.id == id).cloned().or_else(|| i.users.get(id).cloned());
                let other = users.iter().find(|u| u.id != me_id).map(user_name).unwrap_or_else(|| "them".into());
                let mut rows = vec![Row::Start {
                    icon: "lock",
                    title: other.clone(),
                    body: format!(
                        "This is the start of your private conversation with {other}. It's end-to-end encrypted: only your devices and theirs can read it."
                    ),
                }];
                for item in i.dms.items.get(&conversation).into_iter().flatten() {
                    let name = person(&item.sender_id).map(|u| user_name(&u)).unwrap_or_else(|| "Someone".into());
                    match item.kind {
                        ItemKind::Text if !item.deleted => rows.push(Row::Msg(Box::new(Msg {
                            id: item.seq.to_string(),
                            user: person(&item.sender_id),
                            name,
                            color: None,
                            content: item.content.clone(),
                            at: item.at,
                            edited: item.edited_at > 0,
                            head: true,
                            mine: item.sender_id == me_id,
                            pending: false,
                            failed: None,
                            nonce: 0,
                            unreadable: false,
                        }))),
                        ItemKind::Text => {}
                        ItemKind::Unreadable => rows.push(Row::Msg(Box::new(Msg {
                            id: item.seq.to_string(),
                            user: person(&item.sender_id),
                            name,
                            color: None,
                            content: "This message can't be opened on this device.".into(),
                            at: item.at,
                            edited: false,
                            head: true,
                            mine: item.sender_id == me_id,
                            pending: false,
                            failed: None,
                            nonce: 0,
                            unreadable: true,
                        }))),
                        ItemKind::Joined => rows.push(Row::Note {
                            id: item.seq.to_string(),
                            icon: "shield-check",
                            text: "This device joined the conversation. What came before stays on the devices that were here."
                                .into(),
                        }),
                        // The devices a conversation started with aren't news.
                        ItemKind::Devices if !rows.iter().any(|r| matches!(r, Row::Msg(_))) => {}
                        ItemKind::Devices => {
                            let added = item.added.iter().filter(|d| d.user_id != me_id || d.device_id != i.dms.device_id);
                            let mut lines: Vec<String> = Vec::new();
                            for d in added {
                                let who = person(&d.user_id).map(|u| user_name(&u)).unwrap_or_else(|| "Someone".into());
                                lines.push(format!("{who} added a device"));
                            }
                            for d in &item.removed {
                                let who = person(&d.user_id).map(|u| user_name(&u)).unwrap_or_else(|| "Someone".into());
                                lines.push(format!("{who} removed a device"));
                            }
                            if !lines.is_empty() {
                                lines.dedup();
                                rows.push(Row::Note { id: item.seq.to_string(), icon: "laptop", text: lines.join(" · ") });
                            }
                        }
                    }
                }
                for (n, text) in i.dms.sending.get(&conversation).into_iter().flatten().enumerate() {
                    rows.push(Row::Msg(Box::new(Msg {
                        id: format!("s{n}"),
                        user: me.clone(),
                        name: me.as_ref().map(user_name).unwrap_or_default(),
                        color: None,
                        content: text.clone(),
                        at: crate::core::dms::now_ms(),
                        edited: false,
                        head: true,
                        mine: true,
                        pending: true,
                        failed: None,
                        nonce: 0,
                        unreadable: false,
                    })));
                }
                group(&mut rows);
                rows
            }),
            None => Vec::new(),
        }
    }

    /// Keeps the message list in step: grows at the bottom (and lets new
    /// messages rise in), grows at the top for older ones, or starts over.
    pub(crate) fn sync_list(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows();
        let target = self.target().map(|t| t.id());
        let mut h = DefaultHasher::new();
        for r in &rows {
            r.digest(&mut h);
        }
        let digest = h.finish();
        let first = rows.iter().find(|r| matches!(r, Row::Msg(_) | Row::Note { .. })).map(Row::id).unwrap_or_default();
        let len = rows.len();
        let list = &mut self.list;
        if list.target != target {
            self.scroller.update(cx, |s, cx| {
                s.reset(len, cx);
                s.scroll_to_end(cx);
            });
            self.fresh.clear();
        } else if list.digest != digest {
            if len > list.len && first == list.first {
                let added = len - list.len;
                let now = Instant::now();
                for r in &rows[list.len.saturating_sub(1)..] {
                    if matches!(r, Row::Msg(_) | Row::Note { .. }) && !self.fresh.contains_key(&r.id()) {
                        self.fresh.insert(r.id(), now);
                    }
                }
                // Only what's actually new rises in.
                let old: std::collections::HashSet<String> =
                    rows[..list.len.min(rows.len())].iter().map(Row::id).collect();
                self.fresh.retain(|id, at| !old.contains(id) || at.elapsed() < Duration::from_millis(900));
                self.scroller.update(cx, |s, cx| {
                    s.append(added, cx);
                    s.remeasure(cx);
                });
            } else if len > list.len && first != list.first {
                let added = len - list.len;
                self.scroller.update(cx, |s, cx| {
                    s.prepend(added, cx);
                    s.remeasure(cx);
                });
            } else if len == list.len {
                self.scroller.update(cx, |s, cx| s.remeasure(cx));
            } else {
                self.scroller.update(cx, |s, cx| s.reset(len, cx));
            }
        }
        self.list.target = target;
        self.list.len = len;
        self.list.first = first;
        self.list.digest = digest;
        self.fresh.retain(|_, at| at.elapsed() < Duration::from_secs(2));
        self.rows = Rc::new(rows);
    }

    pub(crate) fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let body: AnyElement = match self.nav.clone() {
            Nav::Server { key, server } => self.channel_view(&key, &server, window, cx),
            Nav::Home { dm: Some((key, id)) } => self.dm_view(&key, &id, window, cx),
            Nav::Home { dm: None } => home_splash(&p).into_any_element(),
            Nav::Instance { key } => self.instance_page(&key, window, cx),
        };
        div().flex_1().h_full().min_w_0().flex().bg(p.background).child(body)
    }

    // ───────────────────────── A channel ─────────────────────────

    fn channel_view(&mut self, key: &str, server: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let channel = self
            .channel_in(key, server)
            .and_then(|id| self.core.shared.read(|s| s.instance(key).and_then(|i| i.channel(server, &id).cloned())));
        let Some(channel) = channel else {
            let loading = self.core.shared.read(|s| s.instance(key).is_some_and(|i| !i.channels.contains_key(server)));
            return empty_state(
                &p,
                if loading { "loader-circle" } else { "hash" },
                if loading { "Getting the server ready…" } else { "No text channels here yet" },
                if loading { "" } else { "Someone who can manage channels can add one from the web app." },
            )
            .into_any_element();
        };
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(20.0))
            .border_b_1()
            .border_color(p.border)
            .child(icon("hash").size(px(20.0)).text_color(p.muted_foreground))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(channel.name.clone()))
            .when(!channel.topic.is_empty(), |el| {
                el.child(div().w(px(1.0)).h(px(20.0)).bg(p.border)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(channel.topic.clone()),
                )
            })
            .when(channel.topic.is_empty(), |el| el.child(div().flex_1()))
            .child(
                icon_button("members-toggle", "users", &p)
                    .when(self.members_open, |el| el.text_color(p.primary).bg(alpha(p.primary, 0.12)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.members_open = !this.members_open;
                        cx.notify();
                    })),
            );

        let column = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(self.message_list(window, cx))
            .child(self.composer_bar(None, window, cx));

        let mut view = div().size_full().flex().child(column);
        if self.members_open {
            view = view.child(self.members_panel(key, server, window, cx));
        }
        view.into_any_element()
    }

    fn message_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let rows = self.rows.clone();
        let fresh = Rc::new(self.fresh.clone());
        let this = cx.entity().downgrade();
        let compact = self.prefs.density == Density::Compact;
        let target = self.list.target.clone().unwrap_or_default();
        let loading = rows.is_empty();
        div().flex_1().min_h_0().relative().child(if loading {
            crate::ui::sidebar::loading_rows(&p).into_any_element()
        } else {
            MessageScroller::new(
                SharedString::from(format!("list|{target}")),
                self.scroller.clone(),
                move |ix, _window, cx| match rows.get(ix) {
                    Some(row) => render_row(row, ix, &fresh, &this, compact, cx),
                    None => div().into_any_element(),
                },
            )
            .with_bottom_fade(p.background)
            .with_row_style(gpui_kit::StyleRefinement::default().pb(px(0.0)).px(px(0.0)))
            .with_list_style(gpui_kit::StyleRefinement::default().pb(px(12.0)))
            .with_jump_button_label("Jump to the latest")
            .size_full()
            .into_any_element()
        })
    }

    /// Where you write: grows with what you type, Enter sends, Shift+Enter
    /// adds a line. `blocked` says why you can't send here, if you can't.
    fn composer_bar(
        &mut self,
        blocked: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        let focused = self.composer.read(cx).focus_handle(cx).is_focused(window);
        let typed = !self.composer.read(cx).value().trim().is_empty();
        let ring = motion::follow("composer-ring", if focused { 1.0 } else { 0.0 }, window, cx);
        let ready = motion::follow("composer-send", if typed { 1.0 } else { 0.0 }, window, cx);
        if let Some(reason) = blocked {
            return div()
                .flex_none()
                .px(px(20.0))
                .pb(px(20.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(16.0))
                        .py(px(14.0))
                        .rounded(px(16.0))
                        .bg(alpha(p.muted_foreground, 0.1))
                        .text_sm()
                        .text_color(p.muted_foreground)
                        .child(icon("lock").size(px(16.0)))
                        .child(reason),
                )
                .into_any_element();
        }
        div()
            .flex_none()
            .px(px(20.0))
            .pb(px(20.0))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(8.0))
                    .pl(px(16.0))
                    .pr(px(8.0))
                    .py(px(8.0))
                    .rounded(px(18.0))
                    .bg(p.card)
                    .border_1()
                    .border_color(mix(p.border, p.primary, ring))
                    .shadow(vec![gpui_kit::BoxShadow {
                        color: alpha(p.primary, 0.22 * ring),
                        offset: gpui_kit::point(px(0.0), px(8.0)),
                        blur_radius: px(24.0),
                        spread_radius: px(-8.0),
                        inset: false,
                    }])
                    .child(div().flex_1().min_w_0().py(px(4.0)).child(Textarea::new(&self.composer).appearance(false)))
                    .child(
                        div()
                            .id("send")
                            .size(px(36.0))
                            .flex_none()
                            .rounded(px(12.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(mix(p.muted, p.primary, ready))
                            .text_color(mix(p.muted_foreground, p.primary_foreground, ready))
                            .cursor_pointer()
                            .active(|s| s.top(px(1.0)))
                            .on_click(cx.listener(|this, _, window, cx| this.send_from_button(window, cx)))
                            .child(div().relative().left(px(-3.0 + 3.0 * ready)).child(icon("send").size(px(18.0)))),
                    ),
            )
            .into_any_element()
    }

    fn members_panel(
        &mut self,
        key: &str,
        server: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        let (members, me) = self.core.shared.read(|s| {
            let i = s.instance(key);
            (
                i.and_then(|i| i.members.get(server).cloned()).unwrap_or_default(),
                i.and_then(|i| i.me.as_ref().map(|m| m.id.clone())).unwrap_or_default(),
            )
        });
        let colors: Vec<Option<Hsla>> = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return vec![None; members.len()] };
            members
                .iter()
                .map(|m| m.user.as_ref().and_then(|u| i.name_color(server, &u.id)).map(|c| rgb(c).into()))
                .collect()
        });
        let mut list = div().flex().flex_col().px(px(8.0)).pb(px(12.0)).child(
            div()
                .px(px(8.0))
                .pt(px(18.0))
                .pb(px(6.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(format!("MEMBERS — {}", members.iter().filter(|m| !m.pending).count())),
        );
        for (n, (m, color)) in members.iter().zip(colors).enumerate() {
            if m.pending {
                continue;
            }
            let Some(user) = m.user.clone() else { continue };
            let name = if m.nickname.is_empty() { user_name(&user) } else { m.nickname.clone() };
            let mine = user.id == me;
            let hover = alpha(p.primary, 0.08);
            let (k, uid) = (key.to_owned(), user.id.clone());
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(format!("member|{}", user.id)))
                    .group("member")
                    .h(px(44.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(px(12.0))
                    .hover(move |s| s.bg(hover))
                    .when(!mine, |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| {
                            this.message_person(k.clone(), uid.clone(), window, cx)
                        }))
                    })
                    .child(avatar(Some(&user), 32.0, &p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color.unwrap_or(p.foreground.into()))
                            .child(name),
                    )
                    .when(!mine, |el| {
                        el.child(
                            div()
                                .opacity(0.0)
                                .group_hover("member", |s| s.opacity(1.0))
                                .text_color(p.primary)
                                .child(icon("lock").size(px(14.0))),
                        )
                    }),
                SharedString::from(format!("member-in|{}", user.id)),
                Duration::from_millis((14 * n.min(20)) as u64),
                6.0,
            ));
        }
        div()
            .id("members")
            .w(px(232.0))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(p.sidebar)
            .border_l_1()
            .border_color(p.border)
            .child(list)
    }

    // ───────────────────────── A private conversation ─────────────────────────

    fn dm_view(&mut self, key: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let (other, safety, verified, blocked, joining, status) = self.core.shared.read(|s| {
            let Some(i) = s.instance(key) else { return (None, None, None, None, false, None) };
            let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
            let other = i
                .dms
                .conversations
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.users.iter().find(|u| u.id != me).cloned());
            (
                other,
                i.dms.safety.get(id).cloned(),
                i.dms.verified.get(id).cloned(),
                i.dms.blocked.get(id).cloned(),
                i.dms.joining.contains(id),
                Some(i.dms.status),
            )
        });
        let name = other.as_ref().map(user_name).unwrap_or_else(|| "Someone".into());
        let is_verified = verified.is_some() && verified == safety;
        let changed = verified.is_some() && verified != safety;
        let header = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(20.0))
            .border_b_1()
            .border_color(p.border)
            .child(avatar(other.as_ref(), 28.0, &p))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child(name))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(alpha(p.success, 0.14))
                    .text_color(p.success)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child(icon("lock").size(px(12.0)))
                    .child("End-to-end encrypted"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("safety")
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .h(px(32.0))
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if changed {
                        p.destructive
                    } else if is_verified {
                        p.success
                    } else {
                        p.muted_foreground
                    })
                    .hover({
                        let bg = alpha(p.primary, 0.1);
                        move |s| s.bg(bg)
                    })
                    .on_click(cx.listener({
                        let (key, id) = (key.to_owned(), id.to_owned());
                        move |this, _, window, cx| {
                            this.open_dialog(Dialog::Safety { key: key.clone(), conversation: id.clone() }, window, cx)
                        }
                    }))
                    .child(
                        icon(if is_verified {
                            "shield-check"
                        } else if changed {
                            "shield-alert"
                        } else {
                            "shield"
                        })
                        .size(px(16.0)),
                    )
                    .child(if is_verified {
                        "Verified"
                    } else if changed {
                        "Safety number changed"
                    } else {
                        "Verify"
                    }),
            );
        let blocked = if !status.is_some_and(|s| s.is_ready()) {
            Some("Encrypted messages are still getting ready on this device…".to_owned())
        } else if joining {
            Some("Joining the conversation on this device…".to_owned())
        } else {
            blocked
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(self.message_list(window, cx))
            .child(self.composer_bar(blocked, window, cx))
            .into_any_element()
    }

    // ───────────────────────── An instance ─────────────────────────

    fn instance_page(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some((name, url, node, me, connection, problem, can_create)) = self.core.shared.read(|s| {
            s.instance(key).map(|i| {
                let creation = i.node.as_ref().map(|n| n.server_creation).unwrap_or_default();
                let can_create = creation == pb::ServerCreation::Everyone as i32
                    || creation == pb::ServerCreation::Unspecified as i32
                    || (creation == pb::ServerCreation::Admins as i32 && i.admin);
                (i.name(), i.url.clone(), i.node.clone(), i.me.clone(), i.connection, i.problem.clone(), can_create)
            })
        }) else {
            return div().into_any_element();
        };
        let streamer = self.prefs.streamer_mode;
        let state = match connection {
            Connection::Live => "Connected",
            Connection::Connecting => "Connecting…",
            Connection::Reconnecting => "Reconnecting…",
            Connection::Offline => "Offline",
            Connection::SignedOut => "Signed out",
        };
        let announcement =
            node.as_ref().and_then(|n| n.announcement.as_ref()).filter(|a| !a.text.is_empty()).map(|a| a.text.clone());
        let signed_out = connection == Connection::SignedOut;
        let k = key.to_owned();
        let body = card(&p)
            .w(px(520.0))
            .p(px(28.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        div()
                            .size(px(64.0))
                            .rounded(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(mix(p.card, p.primary, 0.16))
                            .child(fuwa_mark(42.0, &p)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(name))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .text_sm()
                                    .text_color(p.muted_foreground)
                                    .child(conn_dot(connection, &p).border_color(p.card))
                                    .child(state)
                                    .when(!streamer, |el| el.child("·").child(url.clone())),
                            ),
                    ),
            )
            .when_some(announcement, |el, text| {
                el.child(
                    div()
                        .flex()
                        .gap(px(10.0))
                        .px(px(14.0))
                        .py(px(10.0))
                        .rounded(px(12.0))
                        .bg(alpha(p.primary, 0.1))
                        .child(icon("megaphone").size(px(16.0)).text_color(p.primary))
                        .child(div().text_sm().child(text)),
                )
            })
            .when_some(me.filter(|_| !signed_out), |el, me| {
                el.child(div().flex().items_center().gap(px(12.0)).child(avatar(Some(&me), 40.0, &p)).child(
                    div().flex().flex_col().child(div().font_weight(FontWeight::BOLD).child(user_name(&me))).child(
                        div().text_sm().text_color(p.muted_foreground).child(if streamer {
                            "Signed in".to_owned()
                        } else {
                            format!("Signed in as @{}", me.username)
                        }),
                    ),
                ))
            })
            .when_some(error_line(problem.as_deref().filter(|_| connection != Connection::Live), &p), |el, e| {
                el.child(e)
            })
            .child(div().flex().gap(px(10.0)).flex_wrap().map(|el| {
                if signed_out {
                    el.child(primary_button("again", "Sign in again", &p).on_click(cx.listener({
                        let k = k.clone();
                        move |this, _, window, cx| this.reconnect(&k, window, cx)
                    })))
                } else {
                    el.when(can_create, |el| {
                        el.child(primary_button("create", "Make a server", &p).on_click(cx.listener({
                            let k = k.clone();
                            move |this, _, window, cx| {
                                this.open_dialog(Dialog::CreateServer { key: k.clone() }, window, cx)
                            }
                        })))
                    })
                    .child(soft_button("join", "Join with an invite", &p).on_click(cx.listener(
                        {
                            let k = k.clone();
                            move |this, _, window, cx| {
                                this.open_dialog(Dialog::JoinInvite { key: k.clone() }, window, cx)
                            }
                        },
                    )))
                }
            }));
        div()
            .id("instance-page")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(32.0))
            .child(motion::rise(body, SharedString::from(format!("instance|{key}")), Duration::ZERO, 18.0))
            .into_any_element()
    }

    fn send_from_button(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        let text = self.composer.read(cx).value().to_string();
        if !text.trim().is_empty() {
            // The same as pressing Enter.
            self.send_now(window, cx);
        }
    }
}

/// Puts follow-ups from the same person under one header.
fn group(rows: &mut [Row]) {
    let mut last: Option<(String, i64)> = None;
    for row in rows.iter_mut() {
        match row {
            Row::Msg(m) => {
                let author = m.user.as_ref().map(|u| u.id.clone()).unwrap_or_else(|| m.name.clone());
                m.head = !matches!(&last, Some((a, at)) if *a == author && m.at - at < GROUP_MS && m.at >= *at);
                last = Some((author, m.at));
            }
            _ => last = None,
        }
    }
}

fn render_row(
    row: &Row,
    ix: usize,
    fresh: &Rc<std::collections::HashMap<String, Instant>>,
    this: &WeakEntity<FuwaApp>,
    compact: bool,
    cx: &mut App,
) -> AnyElement {
    let p = pal(cx);
    let is_fresh = fresh.contains_key(&row.id());
    let el: AnyElement = match row {
        Row::Older { loading } => {
            let this = this.clone();
            div()
                .flex()
                .justify_center()
                .py(px(16.0))
                .child(soft_button("older", if *loading { "Loading…" } else { "Show older messages" }, &p).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.load_older(cx));
                    },
                ))
                .into_any_element()
        }
        Row::Start { icon: glyph, title, body } => div()
            .px(px(20.0))
            .pt(px(32.0))
            .pb(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .size(px(64.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(alpha(p.primary, 0.14))
                    .text_color(p.primary)
                    .child(icon(glyph).size(px(30.0))),
            )
            .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(title.clone()))
            .child(div().text_color(p.muted_foreground).child(body.clone()))
            .into_any_element(),
        Row::Note { icon: glyph, text, .. } => div()
            .px(px(20.0))
            .py(px(6.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .text_sm()
            .text_color(p.muted_foreground)
            .child(div().w(px(40.0)).flex().justify_center().child(icon(glyph).size(px(16.0)).text_color(p.primary)))
            .child(text.clone())
            .into_any_element(),
        Row::Msg(m) => message(m, &p, this, compact),
    };
    if is_fresh {
        motion::rise(div().child(el), SharedString::from(format!("rise|{}|{ix}", row.id())), Duration::ZERO, 14.0)
            .into_any_element()
    } else {
        el
    }
}

fn message(m: &Msg, p: &Palette, this: &WeakEntity<FuwaApp>, compact: bool) -> AnyElement {
    let hover = alpha(p.foreground, if p.dark { 0.035 } else { 0.03 });
    let gutter = if compact { 0.0 } else { 56.0 };
    let content: AnyElement = if m.unreadable {
        div().italic().text_color(p.muted_foreground).child(m.content.clone()).into_any_element()
    } else {
        TextView::markdown(SharedString::from(format!("md|{}", m.id)), images_as_links(&m.content))
            .selectable(true)
            .style(TextViewStyle { paragraph_gap: gpui_kit::rems(0.35), ..TextViewStyle::default() })
            .w_full()
            .into_any_element()
    };
    let name = div()
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(m.color.unwrap_or(p.foreground.into()))
        .child(m.name.clone());
    let time = div().text_xs().text_color(p.muted_foreground).child(when(m.at));
    let mut body = div().flex_1().min_w_0().flex().flex_col();
    if m.head {
        body = body.child(div().flex().items_baseline().gap(px(8.0)).child(name).child(time));
    }
    body = body.child(
        div()
            .flex()
            .items_baseline()
            .gap(px(6.0))
            .child(div().flex_1().min_w_0().when(m.pending && m.failed.is_none(), |el| el.opacity(0.55)).child(content))
            .when(m.edited, |el| el.child(div().text_xs().text_color(p.muted_foreground).child("(edited)"))),
    );
    if let Some(reason) = &m.failed {
        let (retry, dismiss) = (this.clone(), this.clone());
        let nonce = m.nonce;
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_xs()
                .text_color(p.destructive)
                .child(format!("Couldn't send: {reason}"))
                .child(
                    div()
                        .id(SharedString::from(format!("retry|{nonce}")))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(move |_, _, cx| {
                            let _ = retry.update(cx, |this, cx| this.retry(nonce, cx));
                        })
                        .child("Retry"),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("dismiss|{nonce}")))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(move |_, _, cx| {
                            let _ = dismiss.update(cx, |this, _| {
                                if let Some(Target::Channel { key, channel, .. }) = this.target() {
                                    this.core.dismiss_pending(&key, &channel, nonce);
                                }
                            });
                        })
                        .child("Dismiss"),
                ),
        );
    }

    let left: AnyElement = if compact {
        div().into_any_element()
    } else if m.head {
        div().w(px(gutter)).flex_none().pt(px(2.0)).child(avatar(m.user.as_ref(), 40.0, p)).into_any_element()
    } else {
        div()
            .w(px(gutter))
            .flex_none()
            .pt(px(3.0))
            .text_xs()
            .text_color(p.muted_foreground)
            .opacity(0.0)
            .group_hover("msg", |s| s.opacity(1.0))
            .child(clock(m.at))
            .into_any_element()
    };

    let actions = (m.mine && !m.pending && !m.unreadable).then(|| {
        let this = this.clone();
        let id = m.id.clone();
        div()
            .absolute()
            .right(px(16.0))
            .top(px(-10.0))
            .opacity(0.0)
            .group_hover("msg", |s| s.opacity(1.0))
            .flex()
            .rounded(px(10.0))
            .bg(p.card)
            .border_1()
            .border_color(p.border)
            .child(icon_button(SharedString::from(format!("del|{id}")), "trash", p).on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.delete(id.clone(), cx));
            }))
    });

    div()
        .id(SharedString::from(format!("msg|{}", m.id)))
        .group("msg")
        .relative()
        .flex()
        .px(px(16.0))
        .mx(px(4.0))
        .rounded(px(10.0))
        .when(m.head, |el| el.mt(px(if compact { 4.0 } else { 10.0 })))
        .py(px(if compact { 1.0 } else { 3.0 }))
        .hover(move |s| s.bg(hover))
        .child(left)
        .when(compact && m.head, |el| el.gap(px(8.0)))
        .child(body)
        .when_some(actions, |el, a| el.child(a))
        .into_any_element()
}

/// Home, with nothing open: the cloud bobbing, and what lives here.
fn home_splash(p: &Palette) -> impl IntoElement {
    let bob = fuwa_mark(96.0, p);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.0))
        .child(div().child(bob).with_animation(
            "splash-bob",
            Animation::new(Duration::from_millis(3200)).repeat(),
            |el, t| el.relative().top(px((t * std::f32::consts::TAU).sin() * 6.0)),
        ))
        .child(motion::rise(
            div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child("Your private messages"),
            "splash-title",
            Duration::from_millis(80),
            12.0,
        ))
        .child(motion::rise(
            div()
                .max_w(px(440.0))
                .text_center()
                .text_color(p.muted_foreground)
                .child("Pick a conversation on the left, or open someone from a server's member list. Everything here is end-to-end encrypted: only your devices and theirs can read it."),
            "splash-body",
            Duration::from_millis(160),
            12.0,
        ))
}

/// A quiet message in the middle of a pane.
fn empty_state(p: &Palette, glyph: &str, title: &str, body: &str) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(
            div()
                .size(px(64.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(alpha(p.primary, 0.12))
                .text_color(p.primary)
                .child(icon(glyph).size(px(28.0))),
        )
        .child(div().text_lg().font_weight(FontWeight::EXTRA_BOLD).child(title.to_owned()))
        .when(!body.is_empty(), |el| el.child(div().text_color(p.muted_foreground).child(body.to_owned())))
}
