//! The second column: a server's channels, your direct messages, or an
//! instance's servers, with who you are at the bottom. The highlight under
//! the open row glides from one row to the next.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};

use crate::core::dms::DmStatus;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Menu, Nav};
use crate::ui::arrange::{ChannelDrag, Slot};
use crate::ui::context_menu::MenuOf;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, badge, conn_dot, icon, icon_button, pal, section_label, server_icon};

pub const SIDEBAR: f32 = 248.0;
/// How long a channel that was just dragged into place glows.
const LANDED: Duration = Duration::from_millis(700);

/// A channel's icon in the list.
fn channel_glyph(c: &pb::Channel) -> &'static str {
    match pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text) {
        pb::ChannelType::Voice => "volume-2",
        pb::ChannelType::Announcement => "megaphone",
        pb::ChannelType::Secure => "shield-check",
        _ => "hash",
    }
}
const ROW: f32 = 36.0;
const LABEL: f32 = 34.0;

impl FuwaApp {
    pub(crate) fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let (header, body) = match self.nav.clone() {
            Nav::Server { key, server } => self.server_sidebar(&key, &server, window, cx),
            Nav::Home { dm } => self.dm_sidebar(dm, window, cx),
            Nav::Instance { key } => self.instance_sidebar(&key, window, cx),
        };
        let call_bar = self.call_bar(window, cx);
        div()
            .w(px(SIDEBAR))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(p.side_surface)
            .border_r_1()
            .border_color(p.border)
            .child(
                div()
                    .h(px(56.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(16.0))
                    .gap(px(8.0))
                    .border_b_1()
                    .border_color(p.border)
                    .child(header),
            )
            .child(div().id("sidebar-scroll").flex_1().overflow_y_scroll().px(px(8.0)).pb(px(12.0)).child(body))
            .when_some(call_bar, |el, bar| el.child(bar))
            .child(self.me_panel(window, cx))
    }

    fn server_sidebar(
        &mut self,
        key: &str,
        server_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        let p = pal(cx);
        let (server, channels, unread, muted, access, server_muted) = self.core.shared.read(|s| {
            let i = s.instance(key);
            let now = crate::core::dms::now_ms();
            let channels = i.and_then(|i| i.channels.get(server_id).cloned());
            let muted: std::collections::HashSet<String> = match (i, &channels) {
                (Some(i), Some(list)) => {
                    list.iter().filter(|c| i.is_muted(server_id, &c.id, now)).map(|c| c.id.clone()).collect()
                }
                _ => Default::default(),
            };
            (
                i.and_then(|i| i.server(server_id).cloned()),
                channels,
                i.map(|i| i.unread.clone()).unwrap_or_default(),
                muted,
                i.map(|i| i.access(server_id)).unwrap_or_default(),
                i.is_some_and(|i| crate::core::notifications::is_muted(i.notification_settings(server_id, ""), now)),
            )
        });
        let manage = access.has(pb::Permission::ManageChannels);
        let Some(server) = server else {
            return (div().into_any_element(), div().into_any_element());
        };
        let open = self.channel_in(key, server_id);

        let header = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .w_full()
            .child({
                // The name opens the server's settings, for people who can change any of it.
                let settings = crate::ui::server_settings::can_open(&access);
                let hover = alpha(p.primary, 0.1);
                let (k, sid) = (key.to_owned(), server.id.clone());
                div()
                    .id("server-name")
                    .flex_1()
                    .min_w_0()
                    .h(px(30.0))
                    .px(px(6.0))
                    .ml(px(-6.0))
                    .mr(px(4.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded(corner(8.0))
                    .group("server-name")
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(server.name.clone()),
                    )
                    .when(settings, |el| {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(p.muted_foreground)
                                    .opacity(0.6)
                                    .group_hover("server-name", |s| s.opacity(1.0))
                                    .child(icon("chevron-down").size(px(14.0))),
                            )
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.open_server_settings(&k, &sid, window, cx)),
                            )
                    })
            })
            .child({
                let menu = Menu::Server { key: key.to_owned(), server: server.id.clone() };
                let open = self.menu.as_ref() == Some(&menu);
                icon_button("server-bell", if server_muted { "bell-off" } else { "bell" }, &p)
                    .size(px(28.0))
                    .when(open || server_muted, |el| el.text_color(p.primary))
                    .when(open, |el| el.bg(alpha(p.primary, 0.12)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = if this.menu.as_ref() == Some(&menu) { None } else { Some(menu.clone()) };
                        cx.notify();
                    }))
            })
            .when(manage, |el| {
                el.child(icon_button("new-channel", "plus", &p).size(px(28.0)).on_click(cx.listener({
                    let (key, server) = (key.to_owned(), server.id.clone());
                    move |this, _, window, cx| {
                        let dialog = Dialog::CreateChannel {
                            key: key.clone(),
                            server: server.clone(),
                            parent: String::new(),
                            kind: pb::ChannelType::Text,
                        };
                        this.open_dialog(dialog, window, cx)
                    }
                })))
            })
            .child(icon_button("invite", "user-plus", &p).size(px(28.0)).on_click(cx.listener({
                let server = server.id.clone();
                move |this, _, window, cx| {
                    this.open_dialog(Dialog::Invite { link: None, server: server.clone() }, window, cx)
                }
            })))
            .child(icon_button("leave", "log-out", &p).size(px(28.0)).on_click(cx.listener({
                let (key, server) = (key.to_owned(), server.id.clone());
                move |this, _, window, cx| {
                    this.open_dialog(Dialog::LeaveServer { key: key.clone(), server: server.clone() }, window, cx)
                }
            })))
            .into_any_element();

        // Kept out until they sign in through the server's provider: the way
        // back in, where the channels were.
        if self.sso_locked(key, server_id).is_some() {
            let (k, sid) = (key.to_owned(), server.id.clone());
            let provider = crate::ui::overlay::provider_name(&server.sso_name).to_owned();
            let hover = alpha(p.primary, 0.18);
            let row = div()
                .id("sso-sidebar-locked")
                .mx(px(8.0))
                .mt(px(8.0))
                .px(px(12.0))
                .py(px(10.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(corner(12.0))
                .bg(alpha(p.primary, 0.1))
                .text_color(p.primary)
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                // To the gate, which names the provider's host before anything opens.
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.navigate(Nav::Server { key: k.clone(), server: sid.clone() }, window, cx)
                }))
                .child(icon("lock-keyhole").size(px(16.0)).flex_none())
                .child(div().flex_1().min_w_0().child(format!("Sign in with {provider} to see the channels")));
            let row = motion::rise(row, SharedString::from(format!("sso-row|{key}|{server_id}")), Duration::ZERO, 6.0);
            return (header, div().child(row).into_any_element());
        }
        let Some(channels) = channels else {
            return (header, loading_rows(&p).into_any_element());
        };

        // Channels without a category first, then each category with its own.
        let is_category = |c: &pb::Channel| c.r#type == pb::ChannelType::Category as i32;
        let mut groups: Vec<(Option<&pb::Channel>, Vec<&pb::Channel>)> =
            vec![(None, channels.iter().filter(|c| !is_category(c) && c.parent_id.is_empty()).collect())];
        for cat in channels.iter().filter(|c| is_category(c)) {
            groups.push((Some(cat), channels.iter().filter(|c| c.parent_id == cat.id).collect()));
        }

        let mut rows = div().relative().pt(px(8.0));
        let mut y = 8.0;
        if server.has_welcome_screen {
            let (key, sid) = (key.to_owned(), server.id.clone());
            let hover = alpha(p.primary, 0.08);
            rows = rows.child(
                div()
                    .id("welcome-open")
                    .h(px(ROW))
                    .mx(px(8.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(corner(10.0))
                    .text_color(p.primary)
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_dialog(Dialog::Welcome { key: key.clone(), server: sid.clone() }, window, cx)
                    }))
                    .child(icon("party-popper").size(px(16.0)))
                    .child("Welcome screen"),
            );
            y += ROW;
        }
        let mut highlight = None;
        let mut n = 0;
        // Where each row sits, for dragging channels into order (ui/arrange.rs).
        let mut slots = Vec::new();
        let dragging = if cx.has_active_drag() { self.dragging.clone() } else { None };
        let moving = |c: &pb::Channel| {
            dragging.as_deref().is_some_and(|d| c.id == d || c.parent_id == d && !c.parent_id.is_empty())
        };
        let ghost = |c: &pb::Channel, count: usize| ChannelDrag {
            key: key.to_owned(),
            server: server_id.to_owned(),
            id: c.id.clone(),
            category: is_category(c),
            name: c.name.clone().into(),
            glyph: channel_glyph(c),
            count,
            width: SIDEBAR - 17.0,
            grab: 0.0,
        };
        for (cat, list) in groups {
            if let Some(cat) = cat {
                slots.push(Slot {
                    category: true,
                    id: cat.id.clone(),
                    parent: String::new(),
                    top: y,
                    bottom: y + LABEL,
                });
                let of =
                    MenuOf::Category { key: key.to_owned(), server: server_id.to_owned(), category: cat.id.clone() };
                let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit());
                let label = section_label(cat.name.clone(), &p)
                    .id(SharedString::from(format!("cat|{}", cat.id)))
                    .when(lit, |el| el.text_color(p.foreground))
                    .on_mouse_down(MouseButton::Right, self.right_click(of.clone(), cx))
                    .on_hover(
                        cx.listener(move |this, hovered: &bool, _, _| this.set_hover_target(of.clone(), *hovered)),
                    )
                    .h(px(LABEL))
                    .flex()
                    .items_end()
                    .pr(px(4.0))
                    .when(moving(cat), |el| el.opacity(0.3))
                    .when(manage, |el| {
                        let drag = ghost(cat, list.len());
                        el.cursor_grab().on_drag(drag, |drag, at, _, cx| {
                            cx.new(|_| ChannelDrag { grab: f32::from(at.y), ..drag.clone() })
                        })
                    });
                rows = rows.child(if manage {
                    let (key, server, parent) = (key.to_owned(), server_id.to_owned(), cat.id.clone());
                    label
                        .group("cat")
                        .child(div().flex_1())
                        .child(
                            div()
                                .id(SharedString::from(format!("cat-add|{}", cat.id)))
                                .opacity(0.0)
                                .group_hover("cat", |s| s.opacity(1.0))
                                .cursor_pointer()
                                .hover({
                                    let fg = p.primary;
                                    move |s| s.text_color(fg)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let dialog = Dialog::CreateChannel {
                                        key: key.clone(),
                                        server: server.clone(),
                                        parent: parent.clone(),
                                        kind: pb::ChannelType::Text,
                                    };
                                    this.open_dialog(dialog, window, cx)
                                }))
                                .child(icon("plus").size(px(14.0))),
                        )
                        .into_any_element()
                } else {
                    label.into_any_element()
                });
                y += LABEL;
            }
            for c in list {
                let active = open.as_deref() == Some(c.id.as_str());
                if active {
                    highlight = Some(y);
                }
                slots.push(Slot {
                    category: false,
                    id: c.id.clone(),
                    parent: c.parent_id.clone(),
                    top: y,
                    bottom: y + ROW - 2.0,
                });
                let quiet = muted.contains(&c.id);
                let count = if quiet { 0 } else { unread.get(&c.id).copied().unwrap_or(0) };
                let row = self
                    .channel_row(key, server_id, c, active, count, &p, cx)
                    .when(quiet && !active, |el| {
                        el.opacity(0.5).child(icon("bell-off").size(px(13.0)).text_color(p.muted_foreground))
                    })
                    .when(moving(c), |el| el.opacity(0.3))
                    .when(manage, |el| {
                        let drag = ghost(c, 0);
                        el.on_drag(drag, |drag, at, _, cx| {
                            cx.new(|_| ChannelDrag { grab: f32::from(at.y), ..drag.clone() })
                        })
                    });
                // Just dropped here: it glows, then settles.
                let landed = self.landed.as_ref().filter(|(id, at)| *id == c.id && at.elapsed() < LANDED);
                let row = match landed {
                    Some((_, at)) => {
                        let glow = p.primary;
                        row.with_animation(
                            SharedString::from(format!("landed|{}|{:?}", c.id, at)),
                            Animation::new(LANDED).with_easing(gpui_kit::ease_out_quint()),
                            move |el, t| el.bg(alpha(glow, 0.32 * (1.0 - t))),
                        )
                        .into_any_element()
                    }
                    None => row.into_any_element(),
                };
                rows = rows.child(motion::rise(
                    div().child(row),
                    SharedString::from(format!("ch|{}|{server_id}|{}", key, c.id)),
                    Duration::from_millis(18 * n),
                    6.0,
                ));
                n += 1;
                y += ROW;
                if let Some((people, height)) = self.voice_people(key, server_id, &c.id, window, cx) {
                    rows = rows.child(people);
                    y += height;
                }
            }
        }
        if let Some(target) = highlight {
            let at = motion::follow(SharedString::from(format!("hl|{key}|{server_id}")), target, window, cx);
            rows = div()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(at))
                        .h(px(ROW - 2.0))
                        .rounded(corner(10.0))
                        .bg(alpha(p.primary, 0.16)),
                )
                .child(rows);
        }
        self.arrange_slots = slots;
        if !cx.has_active_drag() {
            self.arrange = None;
            self.dragging = None;
        }
        let marks = self.arrange_marks(key, server_id, window, cx);
        let list =
            div().id("channel-list").relative().child(rows).when_some(marks, |el, m| el.child(m)).when(manage, |el| {
                el.on_drag_move::<ChannelDrag>(cx.listener(|this, event, _, cx| this.drag_moved(event, cx)))
                    .on_drop::<ChannelDrag>(cx.listener(|this, drag, _, cx| this.drag_dropped(drag, cx)))
            });
        (header, list.into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn channel_row(
        &self,
        key: &str,
        server: &str,
        c: &pb::Channel,
        active: bool,
        unread: u32,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let kind = pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text);
        let (glyph, voice) = (channel_glyph(c), kind == pb::ChannelType::Voice);
        // The voice channel you're in reads as the one you're in.
        let joined = voice
            && self.core.call().is_some_and(|v| v.instance == key && v.server_id == server && v.channel_id == c.id);
        let strong = active || unread > 0 || joined;
        let hover = alpha(p.primary, 0.08);
        // Who's in a voice channel; they're listed under it too.
        let in_voice = if kind == pb::ChannelType::Voice {
            self.core.shared.read(|s| {
                s.instance(key).map(|i| crate::core::calls::in_channel(&i.voice, server, &c.id).len()).unwrap_or(0)
            })
        } else {
            0
        };
        let of = MenuOf::Channel { key: key.to_owned(), server: server.to_owned(), channel: c.id.clone() };
        let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit());
        div()
            .id(SharedString::from(format!("row|{}", c.id)))
            .relative()
            .when(lit, |el| el.bg(hover))
            .on_mouse_down(MouseButton::Right, self.right_click(of.clone(), cx))
            .on_hover(cx.listener({
                let of = of.clone();
                move |this, hovered: &bool, _, _| this.set_hover_target(of.clone(), *hovered)
            }))
            .h(px(ROW - 2.0))
            .mb(px(2.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(corner(10.0))
            .text_color(if strong { p.foreground } else { p.muted_foreground })
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .when(!voice, |el| {
                let (key, server, id) = (key.to_owned(), server.to_owned(), c.id.clone());
                el.on_click(cx.listener(move |this, _, window, cx| this.open_channel(&key, &server, &id, window, cx)))
            })
            // A voice channel joins it, with sound (src/core/voice).
            .when(voice, |el| {
                let (key, server, id) = (key.to_owned(), server.to_owned(), c.id.clone());
                el.on_click(cx.listener(move |this, _, _, cx| {
                    this.core.join_voice(&key, &server, &id);
                    cx.notify();
                }))
            })
            .child(icon(glyph).size(px(17.0)).text_color(if joined {
                p.success
            } else if active {
                p.primary
            } else {
                p.muted_foreground
            }))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(strong, |el| el.font_weight(FontWeight::BOLD))
                    .child(c.name.clone()),
            )
            .when_some(crate::core::shared::shared_label(c), |el, label| {
                el.child(crate::ui::shared_marks::badge(&c.id, label.text, alpha(p.primary, 0.8)))
            })
            .when(unread > 0 && !active, |el| el.child(badge(unread, p).border_color(p.sidebar)))
            .when(in_voice > 0, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .child(icon("users").size(px(13.0)))
                        .child(in_voice.to_string()),
                )
            })
    }

    fn dm_sidebar(
        &mut self,
        open: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        let p = pal(cx);
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(icon("lock").size(px(16.0)).text_color(p.primary))
            .child(div().font_weight(FontWeight::EXTRA_BOLD).child("Direct messages"))
            .into_any_element();

        struct Row {
            key: String,
            id: String,
            other: Option<pb::User>,
            instance: String,
            unread: u32,
        }
        type Status = (String, DmStatus, Option<String>);
        let (rows, statuses): (Vec<Row>, Vec<Status>) = self.core.shared.read(|s| {
            let mut rows = Vec::new();
            let mut statuses = Vec::new();
            for i in s.order.iter().filter_map(|k| s.instance(k)) {
                statuses.push((i.name(), i.dms.status, i.dms.problem.clone()));
                let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                for c in &i.dms.conversations {
                    rows.push(Row {
                        key: i.key.clone(),
                        id: c.id.clone(),
                        other: c.users.iter().find(|u| u.id != me).cloned().or_else(|| c.users.first().cloned()),
                        instance: i.name(),
                        unread: i.dms.unread.get(&c.id).copied().unwrap_or(0),
                    });
                }
            }
            (rows, statuses)
        });

        let mut list = div().relative().pt(px(8.0));
        let mut highlight = None;
        for (n, row) in rows.iter().enumerate() {
            let active = open.as_ref().is_some_and(|(k, id)| *k == row.key && *id == row.id);
            if active {
                highlight = Some(8.0 + n as f32 * 48.0);
            }
            let name = row.other.as_ref().map(crate::core::store::user_name).unwrap_or_else(|| "Someone".into());
            let hover = alpha(p.primary, 0.08);
            let nav = Nav::Home { dm: Some((row.key.clone(), row.id.clone())) };
            let streamer = self.prefs.streamer_mode;
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(format!("dm|{}|{}", row.key, row.id)))
                    .on_mouse_down(
                        MouseButton::Right,
                        self.right_click(MenuOf::Dm { key: row.key.clone(), conversation: row.id.clone() }, cx),
                    )
                    .when(
                        self.context.as_ref().is_some_and(|m| m.of.lit() == format!("dm|{}|{}", row.key, row.id)),
                        |el| el.bg(hover),
                    )
                    .h(px(46.0))
                    .mb(px(2.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(corner(12.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
                    .child(avatar(row.other.as_ref(), 32.0, &p))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .when(active || row.unread > 0, |el| el.font_weight(FontWeight::BOLD))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(if streamer { "Encrypted".to_owned() } else { row.instance.clone() }),
                            ),
                    )
                    .when(row.unread > 0 && !active, |el| el.child(badge(row.unread, &p).border_color(p.sidebar))),
                SharedString::from(format!("dm-in|{}|{}", row.key, row.id)),
                Duration::from_millis(24 * n as u64),
                6.0,
            ));
        }
        if let Some(target) = highlight {
            let at = motion::follow("hl|dms", target, window, cx);
            list = div()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(at))
                        .h(px(46.0))
                        .rounded(corner(12.0))
                        .bg(alpha(p.primary, 0.16)),
                )
                .child(list);
        }
        if rows.is_empty() {
            list = list.child(
                div()
                    .px(px(10.0))
                    .py(px(16.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child("Nobody yet. Open someone from a server's member list to message them privately."),
            );
        }
        for (name, status, problem) in statuses {
            let line = match status {
                DmStatus::Ready => continue,
                DmStatus::Off => format!("{name}: waiting to connect"),
                DmStatus::Starting => format!("{name}: setting up this device…"),
                DmStatus::Failed => format!("{name}: {}", problem.unwrap_or_else(|| "encryption didn't start".into())),
            };
            list = list.child(
                div()
                    .mt(px(8.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(corner(10.0))
                    .bg(alpha(p.muted_foreground, 0.08))
                    .text_xs()
                    .text_color(p.muted_foreground)
                    .child(line),
            );
        }
        (header, list.into_any_element())
    }

    fn instance_sidebar(
        &mut self,
        key: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        let p = pal(cx);
        let (name, connection, servers, admin) = self.core.shared.read(|s| match s.instance(key) {
            Some(i) => (i.name(), Some(i.connection), i.servers.clone(), i.admin),
            None => (String::new(), None, Vec::new(), false),
        });
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .w_full()
            .when_some(connection, |el, c| el.child(conn_dot(c, &p).border_color(p.sidebar)))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(name),
            )
            .when(admin, |el| {
                // Instance settings, for its admins; the gear turns as you point at it.
                let k = key.to_owned();
                el.child(
                    icon_button("instance-settings", "settings", &p)
                        .size(px(28.0))
                        .group("instance-gear")
                        .on_click(cx.listener(move |this, _, window, cx| this.open_instance_settings(&k, window, cx))),
                )
            })
            .into_any_element();
        let mut list = div().pt(px(8.0)).child(section_label("Your servers here", &p));
        for (n, server) in servers.iter().enumerate() {
            let hover = alpha(p.primary, 0.08);
            let nav = Nav::Server { key: key.to_owned(), server: server.id.clone() };
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(format!("is|{}", server.id)))
                    .h(px(44.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(corner(12.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
                    .child(server_icon(server, 30.0, 10.0, &p))
                    .child(div().flex_1().whitespace_nowrap().text_ellipsis().child(server.name.clone())),
                SharedString::from(format!("is-in|{}", server.id)),
                Duration::from_millis(24 * n as u64),
                6.0,
            ));
        }
        if servers.is_empty() {
            list = list.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child("None yet. Make one or join one with an invite."),
            );
        }
        (header, list.into_any_element())
    }

    /// Who you are, where: your picture, your name, and the way to settings.
    fn me_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let key = match &self.nav {
            Nav::Server { key, .. } | Nav::Instance { key } | Nav::Home { dm: Some((key, _)) } => Some(key.clone()),
            Nav::Home { dm: None } => self.core.shared.read(|s| s.order.first().cloned()),
        };
        let (me, instance, connection, status, presence) = key
            .as_ref()
            .and_then(|k| {
                self.core.shared.read(|s| {
                    s.instance(k).map(|i| (i.me.clone(), i.name(), i.connection, i.status(), i.has("rich-presence")))
                })
            })
            .unwrap_or((
                None,
                String::new(),
                crate::core::store::Connection::Offline,
                pb::PresenceStatus::Online,
                false,
            ));
        let name = me.as_ref().map(crate::core::store::user_name).unwrap_or_else(|| "Not signed in".into());
        let live = connection == crate::core::store::Connection::Live;
        let sub = if self.prefs.streamer_mode {
            "Streamer mode".to_owned()
        } else if live && status != pb::PresenceStatus::Online {
            crate::ui::menus::status_label(status).to_owned()
        } else {
            me.as_ref().map(|m| format!("@{} · {instance}", m.username)).unwrap_or(instance)
        };
        div()
            .h(px(60.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(12.0))
            .bg(alpha(p.rail, 0.6))
            .border_t_1()
            .border_color(p.border)
            .child({
                // Signed in and live, the dot is your status; otherwise it's the connection.
                let dot = if live {
                    crate::ui::menus::status_dot(status, 12.0, true, p.rail, &p)
                } else {
                    conn_dot(connection, &p)
                };
                // An instance without presence has no status to pick.
                let menu = key.clone().filter(|_| me.is_some() && presence).map(|key| Menu::Status { key });
                let open = menu.is_some() && self.menu == menu;
                let hover = alpha(p.primary, 0.08);
                div()
                    .id("me-status")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .p(px(4.0))
                    .ml(px(-4.0))
                    .rounded(corner(12.0))
                    .when(open, |el| el.bg(hover))
                    .when_some(menu, |el, menu| {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Set your status").build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = if this.menu.as_ref() == Some(&menu) { None } else { Some(menu.clone()) };
                                cx.notify();
                            }))
                    })
                    .child(
                        div()
                            .relative()
                            .child(avatar(me.as_ref(), 36.0, &p))
                            .child(div().absolute().right(px(-2.0)).bottom(px(-2.0)).child(dot)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.muted_foreground)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(sub),
                            ),
                    )
            })
            .child(
                icon_button("me-settings", "settings", &p)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
    }
}

/// Soft placeholder rows while a server's channels arrive.
pub fn loading_rows(p: &Palette) -> impl IntoElement {
    let shimmer = alpha(p.muted_foreground, 0.12);
    div().pt(px(12.0)).flex().flex_col().gap(px(10.0)).children((0..6).map(move |n| {
        motion::fade_in(
            div().h(px(18.0)).mx(px(10.0)).w(px(120.0 + 18.0 * ((n * 7) % 5) as f32)).rounded_full().bg(shimmer),
            SharedString::from(format!("skeleton-{n}")),
            Duration::from_millis(300 + 80 * n as u64),
        )
    }))
}
