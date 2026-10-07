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
use crate::ui::theme::corner;
use crate::ui::widgets::{badge, conn_dot, fuwa_mark, initials, pal, server_icon};

pub const RAIL: f32 = 72.0;

/// The web's `.server-icon`: a circle that settles into a rounded square
/// (32% of its size) when open or pointed at.
fn icon_radius(size: f32, square: bool) -> f32 {
    if square { size * 0.32 } else { size / 2.0 }
}

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
        let home_radius = motion::follow("home|r", icon_radius(48.0, home_active || home_hovered), window, cx);
        list = list.child(
            self.rail_item(
                "home".into(),
                home_active,
                dm_unread > 0,
                dm_unread,
                Nav::Home { dm: None },
                div()
                    .size(px(48.0))
                    .rounded(px(home_radius))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.card)
                    .child(fuwa_mark(32.0, &p)),
                "Direct messages",
                48.0,
                window,
                cx,
            ),
        );

        for (n, inst) in instances.iter().enumerate() {
            list = list.child(divider(&p));
            list = list.child(self.instance_chip(inst, n, window, cx));
            for (m, (server, unread)) in inst.servers.iter().enumerate() {
                let active = matches!(&self.nav, Nav::Server { key, server: s } if *key == inst.key && *s == server.id);
                let id = format!("s|{}|{}", inst.key, server.id);
                let hovered = self.hovered.as_deref() == Some(id.as_str());
                let radius = motion::follow(
                    SharedString::from(format!("{id}|r")),
                    icon_radius(48.0, active || hovered),
                    window,
                    cx,
                );
                let face = server_icon(server, 48.0, radius, &p);
                let item = self.rail_item(
                    id.clone(),
                    active,
                    *unread > 0,
                    if active { 0 } else { *unread },
                    Nav::Server { key: inst.key.clone(), server: server.id.clone() },
                    face,
                    &server.name,
                    48.0,
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

        // Adding a server or an instance, under a divider.
        let add_hovered = self.hovered.as_deref() == Some("rail-add");
        let add_radius = motion::follow("rail-add|r", icon_radius(48.0, add_hovered), window, cx);
        let add_turn = motion::follow("rail-add|turn", if add_hovered { 90.0 } else { 0.0 }, window, cx);
        let add = div()
            .id("rail-add")
            .size(px(48.0))
            .rounded(px(add_radius))
            .flex()
            .items_center()
            .justify_center()
            .bg(if add_hovered { p.primary } else { p.card })
            .text_color(if add_hovered { p.primary_foreground } else { p.primary })
            .cursor_pointer()
            .on_hover(cx.listener(|this, on: &bool, _, cx| {
                if *on {
                    this.hovered = Some("rail-add".into());
                } else if this.hovered.as_deref() == Some("rail-add") {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .active(|s| s.opacity(0.92))
            .on_click(cx.listener(|this, _, window, cx| this.open_connect(true, window, cx)))
            .child(
                gpui_kit::svg()
                    .path("icons/plus.svg")
                    .size(px(24.0))
                    .text_color(if add_hovered { p.primary_foreground } else { p.primary })
                    .with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(add_turn.to_radians()))),
            );

        div()
            .w(px(RAIL))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(p.rail_surface)
            .child(div().id("rail-scroll").flex_1().overflow_y_scroll().child(list.child(divider(&p)).child(add)))
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
        size: f32,
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
            .h(px(size))
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
                    .top(px(size / 2.0 - pill / 2.0))
                    .w(px(4.0))
                    .h(px(pill.max(0.0)))
                    .rounded_r(px(4.0))
                    .bg(p.foreground)
                    .opacity((pill / 8.0).clamp(0.0, 1.0)),
            )
            .child(div().relative().child(face.overflow_hidden()))
            .when(count > 0, |el| el.child(div().absolute().right(px(8.0)).bottom(px(-4.0)).child(badge(count, &p))))
            .when(hovered && self.context.is_none(), |el| {
                // Drawn last and over everything, so the rail's scrolling doesn't clip it.
                el.child(
                    div().absolute().left(px(RAIL + 2.0)).top(px(size / 2.0 - 15.0)).child(gpui_kit::deferred(
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
        let hovered = self.hovered.as_deref() == Some(id.as_str());
        let radius =
            motion::follow(SharedString::from(format!("{id}|r")), icon_radius(36.0, active || hovered), window, cx);
        // The web's small chip: 36px on the card, the instance's initials in the muted color.
        let face = div()
            .relative()
            .size(px(36.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            .bg(p.card)
            .text_color(p.muted_foreground)
            .font_weight(FontWeight::EXTRA_BOLD)
            .text_size(px(11.2))
            .when(inst.connection != Connection::Live, |el| el.opacity(0.7))
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
            40.0,
            window,
            cx,
        );
        motion::rise(
            div().relative().child(item).child(
                div()
                    .absolute()
                    .left(px(RAIL / 2.0 + 18.0 - 10.8))
                    .top(px(40.0 - 2.0 - 10.8))
                    .child(conn_dot(inst.connection, &p)),
            ),
            SharedString::from(format!("{id}|in-{n}")),
            Duration::from_millis(30 * n as u64),
            8.0,
        )
    }
}

/// A divider between groups: the web's `my-1 h-0.5 w-8` line in the border color.
fn divider(p: &crate::ui::theme::Palette) -> impl IntoElement {
    div().w(px(32.0)).h(px(2.0)).my(px(4.0)).flex_none().rounded_full().bg(p.border)
}
