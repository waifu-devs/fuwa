//! Live tiles at the top of a server's channel list ("Happening now"), like
//! the web app's `components/LiveTiles.tsx`, over `core/live_tiles.rs`.
//! Quiet by design: at most three, only the kinds the server shows, none
//! while you're on Do not disturb or have the server muted, and each one
//! hides from its menu. The server menu's items (`live_tile_menu_items`)
//! turn them off here, or, for managers, pick the kinds everyone sees.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Rgba,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, linear_color_stop, linear_gradient, px,
    rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::live_tiles::{self, Body, TILE_KINDS, Tile, TileKind};
use crate::pb;
use crate::ui::app::FuwaApp;
use crate::ui::context_menu::{Built, Item, MenuOf, run};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_lg, radius_md, radius_xl};
use crate::ui::widgets::{avatar, icon, pal};

/// The web's Tailwind 4 colors for each kind's gradient (`from-… to-…`).
fn tint(kind: TileKind) -> (Rgba, Rgba) {
    match kind {
        TileKind::Voice => (rgb(0x00bc7d), rgb(0x00bba7)),
        TileKind::Poll => (rgb(0xfe9a00), rgb(0xff6900)),
        TileKind::Thread => (rgb(0x00a6f4), rgb(0x615fff)),
        TileKind::Shared => (rgb(0x8e51ff), rgb(0xad46ff)),
        TileKind::App => (rgb(0x7ccf00), rgb(0x009966)),
    }
}

fn kind_icon(kind: TileKind) -> &'static str {
    match kind {
        TileKind::Voice => "volume-2",
        TileKind::Poll => "chart-column",
        TileKind::Thread => "messages-square",
        TileKind::Shared => "radio-tower",
        TileKind::App => "trophy",
    }
}

/// The web's `rose-500`, for the live dot.
const ROSE: u32 = 0xff2056;

/// A tile's words, and how much of its window is left (for the bar along its bottom).
struct Words {
    title: String,
    line: String,
    action: String,
    left: Option<f32>,
}

fn words(tile: &Tile, now: i64) -> Words {
    match &tile.body {
        Body::Voice { channel_name, user_ids, since, video, screen } => Words {
            title: t_with(
                "tiles.voice.title",
                &[("count", Arg::Num(user_ids.len() as i64)), ("channel", Arg::Str(channel_name))],
            ),
            line: if *screen {
                t("tiles.voice.screen")
            } else if *video {
                t("tiles.voice.video")
            } else {
                t_with("tiles.voice.since", &[("time", Arg::Str(&crate::ui::text::clock(*since)))])
            },
            action: t("tiles.voice.join"),
            left: None,
        },
        Body::Poll { channel_name, question, ends_at, .. } => {
            let left = ends_at.map(|end| (end - now).max(0));
            Words {
                title: t_with("tiles.poll.title", &[("channel", Arg::Str(channel_name))]),
                line: question.clone(),
                action: t("tiles.poll.vote"),
                left: left.map(|l| l as f32 / live_tiles::POLL_SOON_MS as f32),
            }
        }
        Body::Thread { title, .. } => {
            Words { title: title.clone(), line: t("tiles.thread.title"), action: t("tiles.thread.open"), left: None }
        }
        Body::Shared { channel_name, unread, servers } => Words {
            title: t_with("tiles.shared.title", &[("channel", Arg::Str(channel_name))]),
            line: t_with(
                "tiles.shared.text",
                &[("count", Arg::Num(i64::from(*unread))), ("servers", Arg::Num(*servers as i64))],
            ),
            action: t("tiles.shared.open"),
            left: None,
        },
        Body::App(app) => Words {
            title: app.title.clone(),
            line: t_with("tiles.app.by", &[("app", Arg::Str(&app.app))]),
            action: if app.action.is_empty() { t("tiles.app.open") } else { app.action.clone() },
            left: app.progress,
        },
    }
}

/// The poll's "Closes in …" or "Open until it's ended", and who voted.
fn poll_extra(ends_at: Option<i64>, voters: i64, now: i64) -> String {
    let when = match ends_at.map(|end| (end - now).max(0)) {
        None => t("tiles.poll.noEnd"),
        Some(left) => {
            let minutes = ((left + 59_999) / 60_000).max(1);
            if minutes < 60 {
                t_with("tiles.poll.closesMinutes", &[("count", Arg::Num(minutes))])
            } else {
                t_with("tiles.poll.closesHours", &[("count", Arg::Num((minutes as f64 / 60.0).round() as i64))])
            }
        }
    };
    format!("{when} · {}", t_with("tiles.poll.voters", &[("count", Arg::Num(voters))]))
}

/// The "live" dot: a soft ring breathes out of it, unless motion is turned down.
fn live_dot(id: impl Into<SharedString>, window: &Window, cx: &gpui_kit::App) -> AnyElement {
    let ring = div().absolute().inset_0().rounded_full().bg(rgb(ROSE));
    let ring = if cx.reduce_motion() {
        div().into_any_element()
    } else {
        let id: SharedString = id.into();
        motion::ambient(ring, SharedString::from(format!("{id}|ring")), Duration::from_millis(1800), window, |el, t| {
            // easeOut on scale 1 → 2.6 and opacity .55 → 0.
            let e = 1.0 - (1.0 - t).powi(2);
            let grow = 4.0 * 1.6 * e;
            el.left(px(-grow)).top(px(-grow)).right(px(-grow)).bottom(px(-grow)).opacity(0.55 * (1.0 - e))
        })
    };
    div()
        .relative()
        .size(px(8.0))
        .flex_none()
        .child(ring)
        .child(div().absolute().inset_0().rounded_full().bg(rgb(ROSE)))
        .into_any_element()
}

impl FuwaApp {
    /// Whether the instance has live tiles at all, and they're on here.
    fn tiles_on(&self, key: &str, server_id: &str) -> bool {
        let prefs = self.core.prefs();
        !prefs.live_tiles_off
            && !prefs.live_tiles_quiet.contains(server_id)
            && self.core.shared.read(|s| s.instance(key).is_some_and(|i| i.has("live-tiles")))
    }

    /// The tiles a server shows you now, by the core's rules.
    fn shown_tiles(&self, key: &str, server_id: &str) -> Vec<Tile> {
        if !self.tiles_on(key, server_id) {
            return Vec::new();
        }
        let in_channel =
            self.core.call().filter(|c| c.instance == key && c.server_id == server_id).map(|c| c.channel_id);
        let hidden = self.core.prefs().live_tiles_hidden;
        let now = crate::core::dms::now_ms();
        self.core.shared.read(|s| {
            s.instance(key)
                .map(|i| live_tiles::tiles_for(i, server_id, in_channel.as_deref(), &hidden, now))
                .unwrap_or_default()
        })
    }

    /// "Happening now" over a server's channels, or nothing.
    pub(crate) fn live_tiles_strip(
        &mut self,
        key: &str,
        server_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tiles = self.shown_tiles(key, server_id);
        if tiles.is_empty() {
            return None;
        }
        // The threads you follow count new replies once the app knows which they are.
        if self.requested.insert(format!("{key}|{server_id}|followed")) {
            let (core, k, s) = (self.core.clone(), key.to_owned(), server_id.to_owned());
            let id = format!("{key}|{server_id}|followed");
            self.run(cx, async move { core.load_followed(&k, &s).await }, move |this, result, _| {
                if result.is_err() {
                    this.requested.remove(&id);
                }
            });
        }
        // Countdowns are in minutes and apps' tiles run out by the minute: look again then.
        self.tick_tiles(cx);
        let p = pal(cx);
        let now = crate::core::dms::now_ms();
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (n, tile) in tiles.iter().enumerate() {
            let card = self.tile_card(key, server_id, tile, now, window, cx, &p);
            list = list.child(motion::rise(
                div().child(card),
                SharedString::from(format!("tile-in|{key}|{}", tile.id)),
                Duration::from_millis(50 * n as u64),
                -10.0,
            ));
        }
        let strip = div()
            .pt(px(12.0))
            .child(
                div()
                    .mb(px(6.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(11.2))
                    .line_height(px(16.8))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(live_dot(format!("strip|{key}|{server_id}"), window, cx))
                    .child(t("tiles.strip.label").to_uppercase()),
            )
            .child(list);
        Some(
            motion::rise(strip, SharedString::from(format!("tiles|{key}|{server_id}")), Duration::ZERO, -8.0)
                .into_any_element(),
        )
    }

    /// Redraws once a quarter minute while tiles show, as the web's `useNow(15_000)`.
    fn tick_tiles(&mut self, cx: &mut Context<Self>) {
        if self.tiles_ticking {
            return;
        }
        self.tiles_ticking = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(15)).await;
            let _ = this.update(cx, |this, cx| {
                this.tiles_ticking = false;
                cx.notify();
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn tile_card(
        &mut self,
        key: &str,
        server_id: &str,
        tile: &Tile,
        now: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
        p: &Palette,
    ) -> AnyElement {
        let kind = tile.kind();
        let (from, to) = tint(kind);
        let hover_id = format!("tile|{key}|{}", tile.id);
        let of = MenuOf::LiveTile { key: key.to_owned(), server: server_id.to_owned(), tile: tile.id.clone() };
        let menu_open = self.context.as_ref().is_some_and(|m| m.of.lit() == of.lit());
        let hovered = self.hovered.as_deref() == Some(hover_id.as_str()) || menu_open;
        let lift =
            motion::follow(SharedString::from(format!("{hover_id}|y")), if hovered { -1.0 } else { 0.0 }, window, cx);
        let turn =
            motion::follow(SharedString::from(format!("{hover_id}|r")), if hovered { -6.0 } else { 0.0 }, window, cx);
        let nudge =
            motion::follow(SharedString::from(format!("{hover_id}|x")), if hovered { 2.0 } else { 0.0 }, window, cx);
        let w = words(tile, now);

        let open = {
            let (key, server, tile) = (key.to_owned(), server_id.to_owned(), tile.clone());
            move |this: &mut FuwaApp, window: &mut Window, cx: &mut Context<FuwaApp>| {
                this.open_tile(&key, &server, &tile, window, cx)
            }
        };
        let open_a = open.clone();

        // The kind's badge: a gradient square with its icon, which tilts on hover.
        let badge = div()
            .size(px(32.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius_lg())
            .bg(linear_gradient(135.0, linear_color_stop(from, 0.0), linear_color_stop(to, 1.0)))
            .shadow_sm()
            .child(
                gpui_kit::svg()
                    .path(SharedString::from(format!("icons/{}.svg", kind_icon(kind))))
                    .size(px(16.0))
                    .text_color(gpui_kit::white())
                    .with_transformation(
                        gpui_kit::Transformation::rotate(gpui_kit::radians(turn.to_radians()))
                            .with_scaling(gpui_kit::size(1.0 + turn / -60.0, 1.0 + turn / -60.0)),
                    ),
            );
        let head = div()
            .id(SharedString::from(format!("{hover_id}|open")))
            .flex_1()
            .min_w_0()
            .flex()
            .items_start()
            .gap(px(8.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| open(this, window, cx)))
            .child(badge)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .child(w.title.clone()),
                    )
                    .child(self.tile_line(tile, &w, p)),
            );
        let menu_button = {
            let of = of.clone();
            let shown = hovered;
            div()
                .id(SharedString::from(format!("{hover_id}|menu")))
                .size(px(24.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius_md())
                .text_color(p.muted_foreground)
                .opacity(if shown { 1.0 } else { 0.0 })
                .cursor_pointer()
                .hover({
                    let bg = p.muted;
                    move |s| s.bg(bg)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if this.context.as_ref().is_some_and(|m| m.of.lit() == of.lit()) {
                        this.close_context_menu(cx);
                        return;
                    }
                    // The web's `align="end"`: the menu's right edge under the button's.
                    let at = ev.position() + gpui_kit::point(px(-227.0), px(24.0));
                    this.open_context_menu(of.clone(), at, window, cx);
                }))
                .child(icon("ellipsis").size(px(16.0)))
        };

        let extra = self.tile_extra(key, tile, now, window, cx, p);
        let action_bg = alpha(p.primary, 0.12);
        let action_hover = alpha(p.primary, 0.2);
        let action = div()
            .id(SharedString::from(format!("{hover_id}|act")))
            .h(px(24.0))
            .flex_none()
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .rounded(radius_md())
            .bg(action_bg)
            .text_size(px(12.0))
            .line_height(px(16.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.primary)
            .cursor_pointer()
            .hover(move |s| s.bg(action_hover))
            .active(|s| s.opacity(0.9))
            .on_click(cx.listener(move |this, _, window, cx| open_a(this, window, cx)))
            .child(w.action.clone())
            .child(div().relative().left(px(nudge)).child(icon("chevron-right").size(px(14.0))));

        // `bg-card/80` over the sidebar, made solid: GPUI draws a shadow under a see-through fill.
        let card_bg = crate::ui::theme::mix(p.sidebar, p.card, 0.8);
        let hover_key = hover_id.clone();
        let mut card = div()
            .id(SharedString::from(hover_id.clone()))
            .relative()
            .top(px(lift))
            .overflow_hidden()
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .bg(card_bg)
            .shadow_xs()
            .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                if *on {
                    this.hovered = Some(hover_key.clone());
                } else if this.hovered.as_deref() == Some(hover_key.as_str()) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, self.right_click(of, cx));
        if let Body::App(app) = &tile.body
            && app.live
            && !cx.reduce_motion()
        {
            card = card.child(sweep(&hover_id));
        }
        card = card.child(div().flex().items_start().gap(px(8.0)).p(px(8.0)).child(head).child(menu_button));
        if let Body::App(app) = &tile.body
            && !app.rows.is_empty()
        {
            card = card.child(scoreboard(&hover_id, &app.rows, p));
        }
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .pb(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(extra),
                )
                .child(action),
        );
        if let Some(left) = w.left {
            let target = left.clamp(0.0, 1.0);
            let shown = motion::follow(SharedString::from(format!("{hover_id}|left")), target, window, cx);
            card = card.child(
                div().absolute().left_0().right_0().bottom_0().h(px(2.0)).bg(p.muted).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(gpui_kit::relative(shown.clamp(0.0, 1.0)))
                        .bg(linear_gradient(90.0, linear_color_stop(from, 0.0), linear_color_stop(to, 1.0))),
                ),
            );
        }
        card.into_any_element()
    }

    /// The small line under a tile's title.
    fn tile_line(&self, tile: &Tile, w: &Words, p: &Palette) -> AnyElement {
        let line = div().truncate().text_size(px(12.0)).line_height(px(16.0)).text_color(p.muted_foreground);
        match &tile.body {
            Body::App(app) => line
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(icon(if app.webhook { "webhook" } else { "bot" }).size(px(12.0)))
                .child(div().min_w_0().truncate().child(w.line.clone()))
                .into_any_element(),
            _ => line.child(w.line.clone()).into_any_element(),
        }
    }

    /// What sits beside a tile's button: faces, a countdown, a count or a status.
    #[allow(clippy::too_many_arguments)]
    fn tile_extra(
        &self,
        key: &str,
        tile: &Tile,
        now: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
        p: &Palette,
    ) -> AnyElement {
        match &tile.body {
            Body::Voice { user_ids, video, screen, .. } => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(self.faces(key, user_ids, p))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .when(*video, |el| el.child(icon("video").size(px(14.0))))
                        .when(*screen, |el| el.child(icon("monitor-up").size(px(14.0)))),
                )
                .into_any_element(),
            Body::Poll { ends_at, voters, .. } => {
                div().min_w_0().truncate().child(poll_extra(*ends_at, *voters, now)).into_any_element()
            }
            Body::Thread { user_ids, unread, .. } => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .child(self.faces(key, user_ids, p))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(t_with("tiles.thread.unread", &[("count", Arg::Num(i64::from(*unread)))])),
                )
                .into_any_element(),
            Body::Shared { unread, .. } => {
                div().child(if *unread > 99 { "99+".to_owned() } else { unread.to_string() }).into_any_element()
            }
            Body::App(app) => {
                let status = if app.live {
                    [t("tiles.app.live"), app.status.clone()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                } else {
                    app.status.clone()
                };
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .font_weight(FontWeight::BOLD)
                    .when(app.live, |el| el.child(live_dot(format!("tile-live|{}", tile.id), window, cx)))
                    .child(div().min_w_0().truncate().child(status))
                    .into_any_element()
            }
        }
    }

    /// Up to four faces, overlapping, each ringed in the card's color; then a count of the rest.
    fn faces(&self, key: &str, user_ids: &[String], p: &Palette) -> AnyElement {
        let users: Vec<Option<pb::User>> = self.core.shared.read(|s| {
            let i = s.instance(key);
            user_ids.iter().take(4).map(|id| i.and_then(|i| i.users.get(id).cloned())).collect()
        });
        let rest = user_ids.len().saturating_sub(4);
        let mut row = div().flex().flex_none().items_center();
        for (n, user) in users.iter().enumerate() {
            row = row.child(
                div()
                    .size(px(24.0))
                    .flex_none()
                    .when(n > 0, |el| el.ml(px(-10.0)))
                    .my(px(-2.0))
                    .p(px(2.0))
                    .rounded_full()
                    .bg(p.card)
                    .child(avatar(user.as_ref(), 20.0, p)),
            );
        }
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(row)
            .when(rest > 0, |el| el.child(div().font_weight(FontWeight::BOLD).child(format!("+{rest}"))))
            .into_any_element()
    }

    /// Opens a tile's channel; a voice tile joins the call, a thread tile opens the thread.
    fn open_tile(&mut self, key: &str, server: &str, tile: &Tile, window: &mut Window, cx: &mut Context<Self>) {
        match &tile.body {
            Body::Voice { .. } => {
                self.core.join_voice(key, server, &tile.channel_id);
                cx.notify();
            }
            Body::Thread { thread_id, .. } => {
                self.open_channel(key, server, &tile.channel_id, window, cx);
                self.open_thread(thread_id.clone(), window, cx);
            }
            _ => self.open_channel(key, server, &tile.channel_id, window, cx),
        }
    }

    /// A tile's own menu: hide it, quiet the server, take an app's down for everyone, or turn tiles off.
    pub(crate) fn live_tile_items(&self, key: &str, server: &str, tile_id: &str) -> Built {
        let Some(tile) = self.shown_tiles(key, server).into_iter().find(|t| t.id == tile_id) else {
            return Built::of(Vec::new());
        };
        let mut first = Vec::new();
        let id = tile.id.clone();
        first.push(Item::act(
            t("tiles.strip.hide"),
            "eye-off",
            run(move |this, _, _| this.core.set_prefs(|p| live_tiles::hide(&mut p.live_tiles_hidden, &id))),
        ));
        let sid = server.to_owned();
        first.push(Item::act(
            t("tiles.strip.quiet"),
            "bell-off",
            run(move |this, _, _| {
                this.core.set_prefs(|p| {
                    p.live_tiles_quiet.insert(sid.clone());
                })
            }),
        ));
        let access = self.core.shared.read(|s| s.instance(key).map(|i| i.access(server)).unwrap_or_default());
        if live_tiles::removable(&tile, |c| access.has_in(c, pb::Permission::ManageMessages))
            && let Body::App(app) = &tile.body
        {
            let (k, s, c, source, tid) = (
                key.to_owned(),
                server.to_owned(),
                tile.channel_id.clone(),
                app.source_id.clone(),
                app.tile_id.clone(),
            );
            first.push(
                Item::act(
                    t("tiles.strip.remove"),
                    "trash",
                    run(move |this, _, cx| {
                        let (core, k, s, c, source, tid) =
                            (this.core.clone(), k.clone(), s.clone(), c.clone(), source.clone(), tid.clone());
                        this.run(
                            cx,
                            async move { core.end_live_tile(&k, &s, &c, &source, &tid).await },
                            |this, r, cx| {
                                if let Err(err) = r {
                                    let message = if err.message.is_empty() {
                                        t("tiles.strip.removeFailed")
                                    } else {
                                        err.message
                                    };
                                    this.toast("circle-alert", message, String::new(), None, None, cx);
                                }
                            },
                        );
                    }),
                )
                .danger(),
            );
        }
        let off = vec![Item::act(
            t("tiles.strip.off"),
            "power-off",
            run(|this, _, _| this.core.set_prefs(|p| p.live_tiles_off = true)),
        )];
        Built::of(vec![first, off])
    }

    /// In the server's menu: tiles here or not, for you; and for whoever can
    /// manage the server, which kinds everyone here sees (a server setting).
    pub(crate) fn live_tile_menu_items(&self, key: &str, server: &str) -> Vec<Item> {
        let Some((has, setting, members, manage)) = self.core.shared.read(|s| {
            let i = s.instance(key)?;
            let sv = i.server(server)?;
            Some((
                i.has("live-tiles"),
                sv.live_tiles.clone(),
                sv.member_count,
                i.access(server).has(pb::Permission::ManageServer),
            ))
        }) else {
            return Vec::new();
        };
        if !has {
            return Vec::new();
        }
        let prefs = self.core.prefs();
        let on = !prefs.live_tiles_off && !prefs.live_tiles_quiet.contains(server);
        let sid = server.to_owned();
        let mut items = vec![
            Item::check(
                t("tiles.menu.show"),
                on,
                false,
                run(move |this, _, _| {
                    this.core.set_prefs(|p| {
                        if on {
                            p.live_tiles_quiet.insert(sid.clone());
                        } else {
                            p.live_tiles_off = false;
                            p.live_tiles_quiet.remove(&sid);
                        }
                    })
                }),
            )
            .keep_open(),
        ];
        if manage {
            let customized = setting.as_ref().is_some_and(|s| s.customized);
            let kinds = live_tiles::shown_kinds(setting.as_ref(), members);
            let mut sub: Vec<Item> = TILE_KINDS
                .into_iter()
                .map(|kind| {
                    let shown = kinds.contains(&kind);
                    let mut next: HashSet<TileKind> = kinds.clone();
                    if shown {
                        next.remove(&kind);
                    } else {
                        next.insert(kind);
                    }
                    let numbers = live_tiles::kind_numbers(&next);
                    let save = self.save_tile_kinds(key, server, Some(numbers));
                    Item::check(t(&format!("tiles.kind.{}", kind.name())), shown, false, save).keep_open()
                })
                .collect();
            if customized {
                sub.push(Item::act(t("tiles.menu.reset"), "rotate-ccw", self.save_tile_kinds(key, server, None)));
            }
            let note = if customized {
                t("tiles.menu.everyone")
            } else {
                t_with("tiles.menu.bigServer", &[("count", Arg::Num(live_tiles::BIG_SERVER))])
            };
            sub.push(Item::act(note, "info", run(|_, _, _| {})).disabled(true));
            items.push(Item::sub(t("tiles.menu.server"), "radio-tower", sub));
        }
        items
    }

    fn save_tile_kinds(&self, key: &str, server: &str, kinds: Option<Vec<i32>>) -> crate::ui::context_menu::Run {
        let (key, server) = (key.to_owned(), server.to_owned());
        run(move |this, _, cx| {
            let (core, k, s, kinds) = (this.core.clone(), key.clone(), server.clone(), kinds.clone());
            this.run(cx, async move { core.set_server_live_tiles(&k, &s, kinds).await }, |this, r, cx| {
                if let Err(err) = r {
                    let message = if err.message.is_empty() { t("tiles.menu.failed") } else { err.message };
                    this.toast("circle-alert", message, String::new(), None, None, cx);
                }
            });
        })
    }
}

/// One soft light sweeping across a live app tile as it arrives.
fn sweep(id: &str) -> AnyElement {
    let light = |a: f32| -> Hsla { gpui_kit::hsla(0.0, 0.0, 1.0, a) };
    motion::once(
        div().absolute().top_0().bottom_0().w(gpui_kit::relative(0.5)).bg(linear_gradient(
            90.0,
            linear_color_stop(light(0.0), 0.0),
            linear_color_stop(light(0.15), 0.5),
        )),
        SharedString::from(format!("{id}|sweep")),
        Duration::from_millis(1600),
        |el, t| {
            let at = ((t - 0.125) / 0.875).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - at).powi(3);
            el.left(gpui_kit::relative(-0.5 + 2.0 * e)).opacity(if t >= 1.0 { 0.0 } else { 1.0 })
        },
    )
}

/// An app's rows, such as teams and scores.
fn scoreboard(id: &str, rows: &[(String, String)], p: &Palette) -> AnyElement {
    let mut list = div()
        .mx(px(8.0))
        .mb(px(8.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .rounded(radius_lg())
        .bg(alpha(p.muted, 0.5))
        .px(px(8.0))
        .py(px(6.0));
    for (n, (label, value)) in rows.iter().enumerate() {
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                .child(motion::rise(
                    div().font_weight(FontWeight::EXTRA_BOLD).child(value.clone()),
                    SharedString::from(format!("{id}|row{n}|{value}")),
                    Duration::ZERO,
                    8.0,
                )),
        );
    }
    list.into_any_element()
}
