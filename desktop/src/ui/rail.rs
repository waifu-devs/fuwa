//! The far-left rail: home (direct messages), then each instance followed by
//! its servers, then adding an instance and the settings. A pill on the left
//! edge grows with what's under it, as on Discord: tall for the open server,
//! half for the hovered one, a dot for unread ones.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::store::Connection;
use crate::pb;
use crate::ui::app::{FuwaApp, Nav};
use crate::ui::context_menu::MenuOf;
use crate::ui::motion;
use crate::ui::theme::{alpha, corner, mix};
use crate::ui::widgets::{badge, conn_dot, fuwa_mark, icon, initials, pal, server_icon};

pub const RAIL: f32 = 76.0;

struct RailInstance {
    key: String,
    name: String,
    connection: Connection,
    servers: Vec<(pb::Server, u32)>,
}

impl FuwaApp {
    pub(crate) fn render_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let instances: Vec<RailInstance> = self.core.shared.read(|s| {
            s.order
                .iter()
                .filter_map(|k| s.instance(k))
                .map(|i| RailInstance {
                    key: i.key.clone(),
                    name: i.name(),
                    connection: i.connection,
                    servers: i.servers.iter().map(|sv| (sv.clone(), i.server_unread(&sv.id))).collect(),
                })
                .collect()
        });
        let dm_unread: u32 =
            self.core.shared.read(|s| s.instances.values().map(|i| i.dms.unread.values().sum::<u32>()).sum());

        let home_active = matches!(self.nav, Nav::Home { .. } | Nav::Friends { .. });
        let mut list = div().flex().flex_col().items_center().gap(px(8.0)).pt(px(12.0)).pb(px(12.0));

        // Home: the little cloud.
        let home_hovered = self.hovered.as_deref() == Some("home");
        let home_radius = motion::follow("home|r", if home_active || home_hovered { 16.0 } else { 24.0 }, window, cx);
        list = list.child(self.rail_item(
            "home".into(),
            home_active,
            dm_unread > 0,
            dm_unread,
            Nav::Home { dm: None },
            {
                let bg = if home_active { p.primary } else { p.card };
                div().size(px(48.0)).rounded(px(home_radius)).flex().items_center().justify_center().bg(bg).child(
                    if home_active {
                        div().child(fuwa_mark_inverted(30.0, &p)).into_any_element()
                    } else {
                        div().child(fuwa_mark(30.0, &p)).into_any_element()
                    },
                )
            },
            "Direct messages",
            window,
            cx,
        ));

        for (n, inst) in instances.iter().enumerate() {
            list =
                list.child(div().w(px(32.0)).h(px(2.0)).rounded_full().bg(alpha(p.muted_foreground, 0.25)).my(px(2.0)));
            list = list.child(self.instance_chip(inst, n, window, cx));
            for (m, (server, unread)) in inst.servers.iter().enumerate() {
                let active = matches!(&self.nav, Nav::Server { key, server: s } if *key == inst.key && *s == server.id);
                let id = format!("s|{}|{}", inst.key, server.id);
                let hovered = self.hovered.as_deref() == Some(id.as_str());
                let radius = motion::follow(
                    SharedString::from(format!("{id}|r")),
                    if active || hovered { 16.0 } else { 24.0 },
                    window,
                    cx,
                );
                let face = server_icon(server, 48.0, radius, &p);
                let item = self.rail_item(
                    id.clone(),
                    active,
                    *unread > 0,
                    0,
                    Nav::Server { key: inst.key.clone(), server: server.id.clone() },
                    face,
                    &server.name,
                    window,
                    cx,
                );
                list = list.child(motion::rise(
                    div().child(item),
                    SharedString::from(format!("{id}|in")),
                    Duration::from_millis(40 * (n + m) as u64),
                    10.0,
                ));
            }
        }

        // Adding an instance, then settings at the bottom.
        let add = div()
            .id("rail-add")
            .size(px(48.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.card)
            .text_color(p.success)
            .cursor_pointer()
            .hover({
                let (bg, fg) = (p.success, p.card);
                move |s| s.bg(bg).text_color(fg).rounded(corner(16.0))
            })
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(|this, _, window, cx| this.open_connect(true, window, cx)))
            .child(icon("plus").size(px(22.0)));

        let settings = div()
            .id("rail-settings")
            .size(px(48.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.card)
            .text_color(p.muted_foreground)
            .cursor_pointer()
            .hover({
                let (bg, fg) = (alpha(p.primary, 0.16), p.primary);
                move |s| s.bg(bg).text_color(fg).rounded(corner(16.0))
            })
            .active(|s| s.top(px(1.0)))
            .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)))
            .child(icon("settings").size(px(20.0)));

        div()
            .w(px(RAIL))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(p.rail_surface)
            .child(div().id("rail-scroll").flex_1().overflow_y_scroll().child(list.child(add)))
            .child(div().flex().justify_center().py(px(12.0)).child(settings))
    }

    /// One thing on the rail, with its pill, its tooltip-ish name and its badge.
    #[allow(clippy::too_many_arguments)]
    fn rail_item(
        &mut self,
        id: String,
        active: bool,
        unread: bool,
        count: u32,
        nav: Nav,
        face: gpui_kit::Div,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        let of = match &nav {
            Nav::Server { key, server } => Some(MenuOf::Server { key: key.clone(), server: server.clone() }),
            _ => None,
        };
        // A server whose menu is open stays lit as if hovered.
        let lit = of.as_ref().is_some_and(|of| self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit()));
        let hovered = self.hovered.as_deref() == Some(id.as_str()) || lit;
        let pill = motion::follow_bouncy(
            SharedString::from(format!("{id}|pill")),
            if active {
                40.0
            } else if hovered {
                20.0
            } else if unread {
                8.0
            } else {
                0.0
            },
            window,
            cx,
        );
        let hover_id = id.clone();
        let hover_of = of.clone();
        div()
            .id(SharedString::from(id.clone()))
            .relative()
            .w(px(RAIL))
            .h(px(48.0))
            .flex()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(hover_id.clone());
                } else if this.hovered.as_deref() == Some(hover_id.as_str()) {
                    this.hovered = None;
                }
                if let Some(of) = &hover_of {
                    this.set_hover_target(of.clone(), *hovered);
                }
                cx.notify();
            }))
            .when_some(of, |el, of| el.on_mouse_down(gpui_kit::MouseButton::Right, self.right_click(of, cx)))
            .on_click(cx.listener(move |this, _, window, cx| this.navigate(nav.clone(), window, cx)))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(24.0 - pill / 2.0))
                    .w(px(4.0))
                    .h(px(pill.max(0.0)))
                    .rounded_r(px(4.0))
                    .bg(p.foreground)
                    .opacity((pill / 8.0).clamp(0.0, 1.0)),
            )
            .child(div().relative().child(face.overflow_hidden()))
            .when(count > 0, |el| el.child(div().absolute().right(px(10.0)).bottom(px(-2.0)).child(badge(count, &p))))
            .when(hovered && self.context.is_none(), |el| {
                // Drawn last and over everything, so the rail's scrolling doesn't clip it.
                el.child(
                    div().absolute().left(px(RAIL + 2.0)).top(px(9.0)).child(gpui_kit::deferred(
                        gpui_kit::anchored().child(motion::slide_in(
                            div()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(corner(8.0))
                                .bg(p.card)
                                .border_1()
                                .border_color(p.border)
                                .shadow_md()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .whitespace_nowrap()
                                .child(name.to_owned()),
                            SharedString::from(format!("{id}|tip")),
                            -6.0,
                        )),
                    )),
                )
            })
    }

    /// An instance's own little chip: its initials and how its connection is doing.
    fn instance_chip(
        &mut self,
        inst: &RailInstance,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = pal(cx);
        let active = matches!(&self.nav, Nav::Instance { key } if *key == inst.key);
        let id = format!("i|{}", inst.key);
        let face = div()
            .size(px(48.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(corner(14.0))
            .bg(if active { p.primary.into() } else { mix(p.card, p.primary, 0.12) })
            .text_color(if active { p.primary_foreground } else { p.primary })
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(15.0))
            .child(initials(&inst.name));
        let name = inst.name.clone();
        let item = self.rail_item(
            id.clone(),
            active,
            false,
            0,
            Nav::Instance { key: inst.key.clone() },
            face,
            &name,
            window,
            cx,
        );
        motion::rise(
            div()
                .relative()
                .child(item)
                .child(div().absolute().right(px(12.0)).bottom(px(-1.0)).child(conn_dot(inst.connection, &p))),
            SharedString::from(format!("{id}|in-{n}")),
            Duration::from_millis(30 * n as u64),
            8.0,
        )
    }
}

/// The cloud on the primary color: drawn in the page's color with a primary face.
fn fuwa_mark_inverted(size: f32, p: &crate::ui::theme::Palette) -> impl IntoElement {
    div()
        .relative()
        .size(px(size))
        .child(
            gpui_kit::svg().path("fuwa/mark.svg").absolute().inset_0().size(px(size)).text_color(p.primary_foreground),
        )
        .child(gpui_kit::svg().path("fuwa/face.svg").absolute().inset_0().size(px(size)).text_color(p.primary))
}
