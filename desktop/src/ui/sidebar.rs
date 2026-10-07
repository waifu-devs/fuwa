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
use crate::core::friends::{self, FriendsStatus};
use crate::core::i18n::t;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Menu, Nav};
use crate::ui::arrange::{ChannelDrag, Slot};
use crate::ui::context_menu::MenuOf;
use crate::ui::motion;
use crate::ui::rail::RAIL;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, badge, conn_dot, icon, pal, server_icon};

pub const SIDEBAR: f32 = 256.0;
/// How long a channel that was just dragged into place glows.
const LANDED: Duration = Duration::from_millis(700);

/// A channel's icon in the list.
pub(crate) fn channel_glyph(c: &pb::Channel) -> &'static str {
    match pb::ChannelType::try_from(c.r#type).unwrap_or(pb::ChannelType::Text) {
        pb::ChannelType::Voice => "volume-2",
        pb::ChannelType::Announcement => "megaphone",
        pb::ChannelType::Secure => "shield-check",
        _ => "hash",
    }
}
/// A channel row: the web's `row-y` (6px each side) around 15px text, and the 2px gap.
const ROW: f32 = 36.5;
/// A category's header: `mt-4` over a `py-1` 12px line.
const LABEL: f32 = 40.0;

impl FuwaApp {
    pub(crate) fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let (header, body) = match self.nav.clone() {
            Nav::Server { key, server } => self.server_sidebar(&key, &server, window, cx),
            // A conversation or Friends on an instance shows that instance's sidebar, as on the web.
            Nav::Home { dm: Some((key, _)) } | Nav::Friends { key } => self.instance_sidebar(&key, window, cx),
            Nav::Home { dm: None } => self.dm_sidebar(None, window, cx),
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
        // The voice channel open as its stage, or the text channel open.
        let open = self.stage_in(key, server_id).or_else(|| self.channel_in(key, server_id));

        // The web's server menu trigger: the name over "N members · instance",
        // a chevron, and the server's menu under it.
        let node_name = self.core.shared.read(|s| s.instance(key).map(|i| i.name()).unwrap_or_default());
        let members = server.member_count;
        let of = MenuOf::ServerHeader { key: key.to_owned(), server: server.id.clone() };
        let menu_open = self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit());
        let turn = motion::follow(
            SharedString::from(format!("server-chevron|{key}|{server_id}")),
            if menu_open { 180.0 } else { 0.0 },
            window,
            cx,
        );
        let header = {
            let hover = alpha(p.muted, 0.6);
            let of = of.clone();
            div()
                .id("server-name")
                .flex_1()
                .min_w_0()
                .h(px(56.0))
                .mx(px(-16.0))
                .px(px(16.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .when(menu_open, |el| el.bg(hover))
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.context.as_ref().is_some_and(|m| m.of.lit() == of.lit()) {
                        this.context = None;
                        cx.notify();
                    } else {
                        let at = gpui_kit::point(px(RAIL + 8.0), px(56.0 + 4.0));
                        this.open_context_menu(of.clone(), at, window, cx);
                    }
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .text_size(px(16.0))
                                .line_height(px(24.0))
                                .child(server.name.clone()),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(format!(
                                    "{members} {} · {node_name}",
                                    if members == 1 { "member" } else { "members" }
                                )),
                        ),
                )
                .child(
                    gpui_kit::svg()
                        .path("icons/chevron-down.svg")
                        .size(px(16.0))
                        .flex_none()
                        .text_color(p.foreground)
                        .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(turn.to_radians()))),
                )
                .into_any_element()
        };
        let _ = (server_muted, manage);

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

        let mut rows = div().relative().pt(px(12.0));
        let mut y = 12.0;
        if server.has_welcome_screen || server.has_onboarding {
            // Going through the onboarding again, or the welcome screen when there's none.
            let onboarding = server.has_onboarding;
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
                        if onboarding {
                            this.open_onboarding(&key, &sid, cx)
                        } else {
                            this.open_dialog(Dialog::Welcome { key: key.clone(), server: sid.clone() }, window, cx)
                        }
                    }))
                    .child(icon(if onboarding { "sparkles" } else { "party-popper" }).size(px(16.0)))
                    .child(if onboarding { "Channels & roles" } else { "Welcome screen" }),
            );
            y += ROW;
        }
        let mut highlight = None;
        let mut highlight_h = ROW - 2.0;
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
                let closed = self.collapsed.contains(&cat.id);
                let turn = motion::follow(
                    SharedString::from(format!("cat-chev|{}", cat.id)),
                    if closed { -90.0 } else { 0.0 },
                    window,
                    cx,
                );
                let (fg, hover_fg) = (if lit { p.foreground } else { p.muted_foreground }, p.foreground);
                let cat_id = cat.id.clone();
                let label = div()
                    .id(SharedString::from(format!("cat|{}", cat.id)))
                    .group("cat")
                    .h(px(LABEL))
                    .pt(px(16.0))
                    .pr(px(4.0))
                    .flex()
                    .items_center()
                    .on_mouse_down(MouseButton::Right, self.right_click(of.clone(), cx))
                    .on_hover(
                        cx.listener(move |this, hovered: &bool, _, _| this.set_hover_target(of.clone(), *hovered)),
                    )
                    .when(moving(cat), |el| el.opacity(0.3))
                    .when(manage, |el| {
                        let drag = ghost(cat, list.len());
                        el.on_drag(drag, |drag, at, _, cx| {
                            cx.new(|_| ChannelDrag { grab: f32::from(at.y), ..drag.clone() })
                        })
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("cat-fold|{}", cat.id)))
                            .flex_1()
                            .min_w_0()
                            .h(px(24.0))
                            .px(px(4.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .rounded(crate::ui::theme::radius_md())
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .cursor_pointer()
                            .when(lit, |el| el.bg(alpha(p.muted, 0.7)))
                            .hover(move |s| s.text_color(hover_fg))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&cat_id) {
                                    this.collapsed.insert(cat_id.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                gpui_kit::svg()
                                    .path("icons/chevron-down.svg")
                                    .size(px(12.0))
                                    .flex_none()
                                    .text_color(fg)
                                    .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                                        turn.to_radians(),
                                    ))),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(cat.name.to_uppercase()),
                            )
                            .when(closed && !list.is_empty(), |el| {
                                el.child(
                                    div()
                                        .ml(px(2.0))
                                        .px(px(6.0))
                                        .rounded_full()
                                        .bg(p.muted)
                                        .text_size(px(9.92))
                                        .child(list.len().to_string()),
                                )
                            }),
                    );
                rows = rows.child(if manage {
                    let (key, server, parent) = (key.to_owned(), server_id.to_owned(), cat.id.clone());
                    let hover_fg = p.foreground;
                    label
                        .child(
                            div()
                                .id(SharedString::from(format!("cat-add|{}", cat.id)))
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .text_color(p.muted_foreground)
                                .opacity(0.0)
                                .group_hover("cat", |s| s.opacity(1.0))
                                .cursor_pointer()
                                .hover(move |s| s.text_color(hover_fg))
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
            let folded = cat.is_some_and(|c| self.collapsed.contains(&c.id));
            for c in list.iter().copied().filter(|_| !folded) {
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
                    // An open voice channel's highlight takes in its people, as the web's does.
                    if active {
                        highlight_h = ROW - 2.0 + height;
                    }
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
                        .h(px(highlight_h))
                        .rounded(crate::ui::theme::radius_lg())
                        .bg(alpha(p.primary, 0.15)),
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
        // "Happening now" over the channels (live_tiles.rs); the rows' own top padding is its gap.
        let tiles = self.live_tiles_strip(key, server_id, window, cx);
        (header, div().when_some(tiles, |el, t| el.child(t)).child(list).into_any_element())
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
        let strong = active || unread > 0;
        let of = MenuOf::Channel { key: key.to_owned(), server: server.to_owned(), channel: c.id.clone() };
        let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit());
        // The web's ChannelRow: muted text that darkens on hover, bold for
        // unread, bold in the primary color when open.
        let hover = alpha(p.muted, 0.7);
        let color = if active {
            p.primary
        } else if unread > 0 || lit {
            p.foreground
        } else {
            p.muted_foreground
        };
        let fg = p.foreground;
        let (manage, invite) = self.core.shared.read(|s| {
            let a = s.instance(key).map(|i| i.access(server)).unwrap_or_default();
            (
                a.has_in(&c.id, pb::Permission::ManageChannels) || a.has_in(&c.id, pb::Permission::ManageRoles),
                !voice && kind != pb::ChannelType::Secure && a.has_in(&c.id, pb::Permission::CreateInvite),
            )
        });
        let group = SharedString::from(format!("row-g|{}", c.id));
        let row_action = |id: String, name: &str, show: bool, group: SharedString| {
            div()
                .id(SharedString::from(id))
                .size(px(20.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .text_color(p.muted_foreground)
                .when(!show, |el| el.opacity(0.0).group_hover(group, |s| s.opacity(1.0)))
                .hover(move |s| s.text_color(fg))
                .child(icon(name).size(px(14.0)))
        };
        div()
            .id(SharedString::from(format!("row|{}", c.id)))
            .group(group.clone())
            .relative()
            .when(lit, |el| el.bg(hover))
            .on_mouse_down(MouseButton::Right, self.right_click(of.clone(), cx))
            .on_hover(cx.listener({
                let of = of.clone();
                move |this, hovered: &bool, _, _| this.set_hover_target(of.clone(), *hovered)
            }))
            .h(px(ROW - 2.0))
            .mb(px(2.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(crate::ui::theme::radius_lg())
            .text_size(px(15.04))
            .text_color(color)
            .cursor_pointer()
            .when(!active, |el| el.hover(move |s| s.bg(hover).text_color(fg)))
            .when(!voice, |el| {
                let (key, server, id) = (key.to_owned(), server.to_owned(), c.id.clone());
                el.on_click(cx.listener(move |this, _, window, cx| this.open_channel(&key, &server, &id, window, cx)))
            })
            // A voice channel opens its stage, joining as it opens (voice_stage.rs).
            .when(voice, |el| {
                let (key, server, id) = (key.to_owned(), server.to_owned(), c.id.clone());
                el.on_click(cx.listener(move |this, _, window, cx| this.open_stage(&key, &server, &id, window, cx)))
            })
            // The unread dot at the list's edge.
            .when(unread > 0 && !active, |el| {
                el.child(div().absolute().left(px(-8.0)).top(px(13.25)).w(px(4.0)).h(px(8.0)).rounded_r_full().bg(fg))
            })
            .child(
                div()
                    .flex_none()
                    .opacity(if active { 1.0 } else { 0.7 })
                    .group_hover(group.clone(), |s| s.opacity(1.0))
                    .child(icon(glyph).size(px(18.0)).text_color(color)),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(strong, |el| el.font_weight(FontWeight::BOLD))
                    .child(c.name.clone()),
            )
            .when_some(crate::core::shared::shared_label(c), |el, label| {
                el.child(crate::ui::shared_marks::badge(&c.id, label.text, alpha(p.primary, 0.8)))
            })
            .child(div().flex_1())
            .when(invite, |el| {
                let server = server.to_owned();
                let channel = c.id.clone();
                let k = key.to_owned();
                el.child(row_action(format!("row-invite|{}", c.id), "user-plus", active, group.clone()).on_click(
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.invite_to_channel(&k, &server, &channel, window, cx);
                    }),
                ))
            })
            .when(manage, |el| {
                let (k, server, channel) = (key.to_owned(), server.to_owned(), c.id.clone());
                el.child(row_action(format!("row-edit|{}", c.id), "settings", active, group.clone()).on_click(
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_channel_settings(&k, &server, &channel, window, cx);
                    }),
                ))
            })
            .when(unread > 0 && !active, |el| {
                el.child(
                    div()
                        .h(px(20.0))
                        .min_w(px(20.0))
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(p.destructive)
                        .text_color(gpui_kit::white())
                        .text_size(px(11.2))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(if unread > 99 { "99+".to_owned() } else { unread.to_string() }),
                )
            })
    }

    /// The row's invite shortcut: an invite to its server, shown to copy.
    fn invite_to_channel(
        &mut self,
        _key: &str,
        server: &str,
        _channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_dialog(Dialog::Invite { link: None, server: server.to_owned() }, window, cx);
    }

    /// The row's gear: the channel's page in server settings.
    fn open_channel_settings(
        &mut self,
        key: &str,
        server: &str,
        channel: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_server_settings(key, server, window, cx);
        if let Some(view) = &self.server_settings {
            let channel = channel.to_owned();
            view.update(cx, |view, cx| view.edit_channel(channel, false, cx));
        }
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
        /// An instance's Friends row: its key, its name, and requests waiting.
        type FriendsRow = (String, String, usize);
        let now = crate::core::dms::now_ms();
        let (rows, statuses, friends): (Vec<Row>, Vec<Status>, Vec<FriendsRow>) = self.core.shared.read(|s| {
            let mut rows = Vec::new();
            let mut statuses = Vec::new();
            let mut friends = Vec::new();
            for i in s.order.iter().filter_map(|k| s.instance(k)) {
                statuses.push((i.name(), i.dms.status, i.dms.problem.clone()));
                if matches!(i.friends.status, FriendsStatus::Loading | FriendsStatus::Ready) {
                    friends.push((i.key.clone(), i.name(), friends::waiting_for_you(&i.friends.list, now)));
                }
                let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                // Conversations with people you blocked stay out of sight until you unblock them.
                for c in i.dms.conversations.iter().filter(|c| !friends::hidden(i, c)) {
                    rows.push(Row {
                        key: i.key.clone(),
                        id: c.id.clone(),
                        other: c.users.iter().find(|u| u.id != me).cloned().or_else(|| c.users.first().cloned()),
                        instance: i.name(),
                        unread: i.dms.unread.get(&c.id).copied().unwrap_or(0),
                    });
                }
            }
            (rows, statuses, friends)
        });
        let several = friends.len() > 1;
        let mut top = div().flex().flex_col().gap(px(2.0)).pt(px(8.0));
        for (n, (key, instance, waiting)) in friends.into_iter().enumerate() {
            let active = matches!(&self.nav, Nav::Friends { key: k } if *k == key);
            let hover = alpha(p.primary, 0.08);
            let nav = Nav::Friends { key: key.clone() };
            top = top.child(motion::rise(
                div()
                    .id(SharedString::from(format!("friends|{key}")))
                    .h(px(40.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(corner(12.0))
                    .cursor_pointer()
                    .when(active, |el| el.bg(alpha(p.primary, 0.16)))
                    .when(!active, |el| el.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
                    .child(icon("users").size(px(18.0)).text_color(if active { p.primary } else { p.muted_foreground }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::BOLD)
                            .child(if several && !self.prefs.streamer_mode {
                                format!("Friends · {instance}")
                            } else {
                                "Friends".to_owned()
                            }),
                    )
                    .when(waiting > 0, |el| el.child(badge(waiting as u32, &p).border_color(p.sidebar))),
                SharedString::from(format!("friends-in|{key}")),
                Duration::from_millis(24 * n as u64),
                6.0,
            ));
        }

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
                    .child("Nobody yet. Message a friend, or someone from a server's member list."),
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
        (header, div().child(top).child(list).into_any_element())
    }

    /// The web's InstanceSidebar: what the instance is, then Friends, Browse
    /// servers, your direct messages there and your servers there.
    fn instance_sidebar(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        let p = pal(cx);
        let now = crate::core::dms::now_ms();
        let streamer = self.prefs.streamer_mode;
        struct Dm {
            id: String,
            other: Option<pb::User>,
            line: String,
            unread: u32,
            calling: bool,
        }
        let (name, url, connection, servers, admin, friends_on, waiting, dms, dm_status) =
            self.core.shared.read(|s| match s.instance(key) {
                Some(i) => {
                    let me = i.me.as_ref().map(|m| m.id.clone()).unwrap_or_default();
                    let dms: Vec<Dm> = i
                        .dms
                        .conversations
                        .iter()
                        .filter(|c| !friends::hidden(i, c))
                        .map(|c| Dm {
                            id: c.id.clone(),
                            other: c.users.iter().find(|u| u.id != me).cloned().or_else(|| c.users.first().cloned()),
                            line: preview(i.dms.items.get(&c.id).map(Vec::as_slice), &me),
                            unread: i.dms.unread.get(&c.id).copied().unwrap_or(0),
                            calling: i.dms.calls.get(&c.id).is_some_and(|call| !call.participants.is_empty()),
                        })
                        .collect();
                    (
                        i.name(),
                        i.url.clone(),
                        Some(i.connection),
                        i.servers.clone(),
                        i.admin,
                        matches!(i.friends.status, FriendsStatus::Loading | FriendsStatus::Ready),
                        friends::waiting_for_you(&i.friends.list, now),
                        dms,
                        (i.dms.status, i.dms.problem.clone()),
                    )
                }
                None => {
                    (String::new(), String::new(), None, Vec::new(), false, false, 0, Vec::new(), (DmStatus::Off, None))
                }
            });
        let address = url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_owned();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .w_full()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_size(px(16.0))
                            .line_height(px(24.0))
                            .child(name),
                    )
                    .when_some(connection, |el, c| {
                        let label = crate::core::i18n::t(match c {
                            crate::core::store::Connection::Live => "workspace.connection.live",
                            crate::core::store::Connection::Connecting => "workspace.connection.connecting",
                            crate::core::store::Connection::Reconnecting => "workspace.connection.reconnecting",
                            crate::core::store::Connection::Offline => "workspace.connection.offline",
                            crate::core::store::Connection::SignedOut => "workspace.connection.signedOut",
                        });
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(p.muted_foreground)
                                .child(conn_dot(c, &p).size(px(8.8)).border_0())
                                .child(format!("{label} · {}", if streamer { "•••••" } else { address.as_str() })),
                        )
                    }),
            )
            .when(admin, |el| {
                // Instance settings, for its admins; the gear turns as you point at it.
                let k = key.to_owned();
                let (bg, fg) = (p.muted, p.foreground);
                el.child(
                    div()
                        .id("instance-settings")
                        .size(px(32.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(crate::ui::theme::radius_lg())
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(bg).text_color(fg))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_instance_settings(&k, window, cx)))
                        .child(icon("settings").size(px(16.0))),
                )
            })
            .into_any_element();

        // A link at the top: Friends or Browse servers.
        let link =
            |id: &str, glyph: &str, label: String, active: bool, count: usize, nav: Nav, cx: &mut Context<Self>| {
                let (bg, fg) = (p.muted, p.foreground);
                div()
                    .id(SharedString::from(id.to_owned()))
                    .px(px(8.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(crate::ui::theme::radius_lg())
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .when(active, |el| el.bg(alpha(p.primary, 0.15)).text_color(p.primary))
                    .when(!active, |el| el.text_color(p.muted_foreground).hover(move |s| s.bg(bg).text_color(fg)))
                    .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
                    .child(icon(glyph).size(px(16.0)))
                    .child(label)
                    .when(count > 0, |el| {
                        el.child(div().flex_1()).child(
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
                        )
                    })
            };
        let mut body = div().pt(px(8.0)).flex().flex_col();
        if friends_on {
            let active = matches!(&self.nav, Nav::Friends { key: k } if k == key);
            body = body.child(link(
                "friends-link",
                "users",
                t("workspace.instanceSidebar.friends"),
                active,
                waiting,
                Nav::Friends { key: key.to_owned() },
                cx,
            ));
        }
        let browsing = matches!(&self.nav, Nav::Instance { key: k } if k == key) && self.home.invite.is_none();
        body = body.child(link(
            "browse-link",
            "compass",
            t("workspace.instanceSidebar.browse"),
            browsing,
            0,
            Nav::Instance { key: key.to_owned() },
            cx,
        ));

        // Direct messages, private by default.
        let label = |text: String| {
            div()
                .px(px(8.0))
                .mb(px(4.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(text.to_uppercase())
        };
        let emerald = gpui_kit::rgb(0x10b981);
        let mut section = div()
            .mt(px(16.0))
            .child(label(t("dms-calls.dm.list.label")).child(icon("lock-keyhole").size(px(12.0)).text_color(emerald)));
        match dm_status {
            (DmStatus::Failed, problem) => {
                section = section.child(
                    div()
                        .mx(px(8.0))
                        .my(px(4.0))
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(crate::ui::theme::radius_xl())
                        .bg(alpha(gpui_kit::rgb(0xf59e0b), 0.1))
                        .text_xs()
                        .text_color(gpui_kit::rgb(if p.dark { 0xfcd34d } else { 0xb45309 }))
                        .child(problem.unwrap_or_else(|| t("dms-calls.dm.unavailable"))),
                )
            }
            _ if dms.is_empty() => {
                let message = t("dms-calls.dm.list.emptyMessage");
                section = section.child(
                    div()
                        .mx(px(4.0))
                        .mt(px(4.0))
                        .px(px(12.0))
                        .py(px(12.0))
                        .rounded(crate::ui::theme::radius_2xl())
                        .border_1()
                        .border_dashed()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.foreground)
                                .child(
                                    div()
                                        .size(px(24.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(crate::ui::theme::radius_lg())
                                        .bg(alpha(emerald, 0.15))
                                        .text_color(gpui_kit::rgb(if p.dark { 0x34d399 } else { 0x059669 }))
                                        .child(icon("lock-keyhole").size(px(14.0))),
                                )
                                .child(t("dms-calls.dm.list.emptyTitle")),
                        )
                        .child(div().mt(px(6.0)).line_height(px(19.5)).child(crate::ui::text::hint_line(
                            // The template, its placeholder kept for hint_line to fill in bold.
                            &crate::core::i18n::t_with(
                                "dms-calls.dm.list.emptyText",
                                &[("message", crate::core::i18n::Arg::Str("{message}"))],
                            ),
                            &[("message", &message)],
                            &p,
                        ))),
                );
            }
            _ => {}
        }
        let open = match &self.nav {
            Nav::Home { dm: Some((k, id)) } if k == key => Some(id.clone()),
            _ => None,
        };
        for (n, dm) in dms.iter().enumerate() {
            let active = open.as_deref() == Some(dm.id.as_str());
            let who = dm.other.as_ref().map(crate::core::store::user_name).unwrap_or_else(|| t("common.someone"));
            let lit = self.context.as_ref().is_some_and(|m| m.of.lit() == format!("dm|{key}|{}", dm.id));
            let nav = Nav::Home { dm: Some((key.to_owned(), dm.id.clone())) };
            let hover = p.muted;
            let line = if dm.line.is_empty() || streamer { t("dms-calls.dm.encrypted") } else { dm.line.clone() };
            section = section.child(motion::rise(
                div()
                    .id(SharedString::from(format!("dm|{key}|{}", dm.id)))
                    .on_mouse_down(
                        MouseButton::Right,
                        self.right_click(MenuOf::Dm { key: key.to_owned(), conversation: dm.id.clone() }, cx),
                    )
                    .px(px(8.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(crate::ui::theme::radius_lg())
                    .text_size(px(14.0))
                    .cursor_pointer()
                    .when(active, |el| el.bg(alpha(p.primary, 0.15)))
                    .when(lit && !active, |el| el.bg(hover))
                    .when(!active, |el| el.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
                    .child(
                        div().relative().flex_none().child(avatar(dm.other.as_ref(), 32.0, &p)).child(
                            div()
                                .absolute()
                                .right(px(-2.0))
                                .bottom(px(-2.0))
                                .size(px(14.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(p.card)
                                .text_color(emerald)
                                .child(icon("lock-keyhole").size(px(10.0))),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_weight(FontWeight::BOLD)
                                    .line_height(px(20.0))
                                    .child(who),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .when(dm.unread > 0, |el| {
                                        el.font_weight(FontWeight::BOLD).text_color(alpha(p.foreground, 0.8))
                                    })
                                    .when(dm.unread == 0, |el| el.text_color(p.muted_foreground))
                                    .child(line),
                            ),
                    )
                    .when(dm.calling, |el| {
                        el.child(
                            div()
                                .size(px(24.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(gpui_kit::rgb(0x3ba55d))
                                .text_color(gpui_kit::white())
                                .child(icon("phone-call").size(px(14.0))),
                        )
                    })
                    .when(dm.unread > 0, |el| {
                        el.child(
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
                                .child(if dm.unread > 99 { "99+".to_owned() } else { dm.unread.to_string() }),
                        )
                    }),
                SharedString::from(format!("dm-in|{key}|{}", dm.id)),
                Duration::from_millis(30 * n.min(12) as u64),
                6.0,
            ));
        }
        body = body.child(section);

        if !servers.is_empty() {
            body = body.child(div().mt(px(16.0)).child(label(t("workspace.instanceSidebar.yourServers"))));
        }
        for (n, server) in servers.iter().enumerate() {
            let hover = p.muted;
            let nav = Nav::Server { key: key.to_owned(), server: server.id.clone() };
            let group = SharedString::from(format!("is-g|{}", server.id));
            body = body.child(motion::rise(
                div()
                    .id(SharedString::from(format!("is|{}", server.id)))
                    .group(group.clone())
                    .px(px(8.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(crate::ui::theme::radius_lg())
                    .text_size(px(14.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
                    .child(server_icon(server, 28.0, 14.0, &p).text_size(px(10.4)))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_weight(FontWeight::BOLD)
                            .child(server.name.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .opacity(0.0)
                            .group_hover(group, |s| s.opacity(1.0))
                            .text_color(p.muted_foreground)
                            .child(icon("hash").size(px(14.0))),
                    ),
                SharedString::from(format!("is-in|{}", server.id)),
                Duration::from_millis(30 * n.min(12) as u64),
                6.0,
            ));
        }
        let _ = window;
        (header, body.into_any_element())
    }

    /// Who you are, where: your picture, your name, and the way to settings.
    fn me_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let key = match &self.nav {
            Nav::Server { key, .. }
            | Nav::Instance { key }
            | Nav::Friends { key }
            | Nav::Home { dm: Some((key, _)) } => Some(key.clone()),
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
        // The web's UserPanel: your custom status, or what you picked, or your username (masked in streamer mode).
        let sub = if live && status != pb::PresenceStatus::Online {
            crate::ui::user_menu::status_name(status)
        } else {
            let _ = &instance;
            me.as_ref()
                .and_then(|m| {
                    crate::ui::presence::custom_status(m, crate::core::dms::now_ms()).or_else(|| {
                        Some(if self.prefs.streamer_mode {
                            format!("@{}", crate::core::accounts::mask_name(&m.username))
                        } else {
                            format!("@{}", m.username)
                        })
                    })
                })
                .unwrap_or_default()
        };
        let (muted, deaf) = self.core.selves();
        // The web's UserPanel: you (opening your status), then mute, deafen and settings.
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(8.0))
            .bg(alpha(p.background, 0.5))
            .border_t_1()
            .border_color(p.border)
            .child({
                // Signed in and live, the dot is your status; otherwise it's the connection.
                let dot = if live && presence {
                    crate::ui::user_menu::presence_dot(status, 11.2, 3.0, p.card.into(), &p)
                        .absolute()
                        .right(px(-5.0))
                        .bottom(px(-5.0))
                } else {
                    div().absolute().right(px(-2.0)).bottom(px(-2.0)).child(conn_dot(connection, &p))
                };
                // Your status, custom status and accounts (user_menu.rs).
                let menu = key.clone().filter(|_| me.is_some()).map(|key| Menu::Status { key });
                let open = menu.is_some() && self.menu == menu;
                let hover = p.muted;
                div()
                    .id("me-status")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(4.0))
                    .rounded(crate::ui::theme::radius_xl())
                    .when(open, |el| el.bg(hover))
                    .when_some(menu, |el, menu| {
                        el.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = if this.menu.as_ref() == Some(&menu) { None } else { Some(menu.clone()) };
                            cx.notify();
                        }))
                    })
                    .child(div().relative().child(avatar(me.as_ref(), 32.0, &p)).child(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::BOLD)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(sub),
                            ),
                    )
            })
            .child(panel_toggle("me-mute", if muted { "mic-off" } else { "mic" }, muted, &p).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.core.set_self_mute(!muted);
                    cx.notify();
                },
            )))
            .child(panel_toggle("me-deafen", if deaf { "headphone-off" } else { "headphones" }, deaf, &p).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.core.set_self_deaf(!deaf);
                    cx.notify();
                }),
            ))
            .child(
                panel_toggle("me-settings", "settings", false, &p)
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
    }
}

/// The web's small panel buttons (`size-8 rounded-lg`, an 18px icon): muted
/// until hovered, red on the destructive tint while on.
fn panel_toggle(id: &'static str, name: &str, on: bool, p: &Palette) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (hover_bg, hover_fg) =
        if on { (alpha(p.destructive, 0.2), p.destructive) } else { (p.muted.into(), p.foreground) };
    div()
        .id(id)
        .size(px(32.0))
        .flex_none()
        .rounded(crate::ui::theme::radius_lg())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(on, |el| el.bg(alpha(p.destructive, 0.12)).text_color(p.destructive))
        .when(!on, |el| el.text_color(p.muted_foreground))
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .active(|s| s.opacity(0.85))
        .child(icon(name).size(px(18.0)))
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

/// What a conversation's last line says, for the list: read from this
/// device's copy, never the instance (the web's DmList `preview`).
fn preview(items: Option<&[crate::core::vault::Item]>, me: &str) -> String {
    use crate::core::vault::ItemKind;
    let Some(items) = items else { return String::new() };
    for item in items.iter().rev() {
        if item.kind != ItemKind::Text {
            continue;
        }
        if item.deleted {
            return t("dms-calls.dm.deleted");
        }
        let text: String = item.content.chars().filter(|c| !matches!(c, '*' | '_' | '~' | '`' | '>' | '#')).collect();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        return if item.sender_id == me {
            crate::core::i18n::t_with("dms-calls.dm.list.youSaid", &[("text", crate::core::i18n::Arg::Str(&text))])
        } else {
            text
        };
    }
    String::new()
}
