//! An instance's front page: its welcome (how it's connected, its name,
//! making a server), opening an invite from any instance, and Browse, the
//! servers anyone here can find, each with the one button that gets you in
//! (open, join, apply, waiting, single sign-on, waifu.dev only). Like the
//! web app's `pages/InstanceHome.tsx`, `components/join/JoinButton.tsx` and
//! `ServerDoor.tsx`. Invites, applying and making servers are in
//! `ui/join.rs`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, BoxShadow, Context, Div, Entity, FontStyle, FontWeight, HighlightStyle,
    Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Rgba, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task, Window, canvas, div, fill, hsla,
    point, px, size,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::join::{self, Kind};
use crate::core::store::Connection;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Nav};
use crate::ui::motion;
use crate::ui::text::{TIGHT, Tracked, tracked};
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_3xl, radius_xl};
use crate::ui::widgets::{icon, pal, server_icon};

/// The rail's and the sidebar's widths, beside the page.
const BESIDE: f32 = 72.0 + crate::ui::sidebar::SIDEBAR;

/// What the instance page holds between draws.
pub struct Home {
    pub search: Entity<InputState>,
    pub invite_field: Entity<InputState>,
    /// The pasted text didn't read as an invite: says so, and shakes.
    invite_bad: bool,
    shakes: u32,
    /// The instance whose Browse is showing, so coming back asks again.
    shown: Option<String>,
    browse: HashMap<String, Browse>,
    /// Joins on their way, joins that just went through (a tick, then the
    /// server opens), and what went wrong, by "key|server".
    joining: HashSet<String>,
    joined: HashSet<String>,
    errors: HashMap<String, String>,
    pub(crate) withdrawing: HashSet<String>,
    /// Servers whose provider has the browser open now, and ones signed in through lately.
    sso_waiting: Option<String>,
    sso_done: HashSet<String>,
    /// Servers seen in Browse or on an invite, by "key|server", for the dialogs about them.
    pub(crate) known: HashMap<String, pb::Server>,
    /// The invite being looked at, in place of Browse (`ui/join.rs`).
    pub invite: Option<crate::ui::join::InvitePage>,
    /// Applying to a server (`ui/join.rs`).
    pub apply: Option<crate::ui::join::ApplyForm>,
    /// Making a server (`ui/join.rs`).
    pub create: crate::ui::join::CreateForm,
    /// Asks how your applications went every half a minute.
    _watch: Task<()>,
}

/// Browse on one instance.
#[derive(Clone)]
enum Browse {
    Loading,
    Ready(Vec<pb::Server>),
    Failed(String),
}

/// How often to ask whether anyone looked at your applications.
const EVERY: Duration = Duration::from_secs(30);

impl Home {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("workspace.home.search")));
        let invite_field = cx.new(|cx| InputState::new(window, cx).placeholder("https://chat.example.com/invite/…"));
        let (create, mut subs) = crate::ui::join::CreateForm::new(window, cx);
        subs.push(cx.subscribe_in(&search, window, |_: &mut FuwaApp, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&invite_field, window, |this: &mut FuwaApp, _, event: &InputEvent, window, cx| {
            match event {
                InputEvent::PressEnter { .. } => this.open_typed_invite(window, cx),
                InputEvent::Change => {
                    this.home.invite_bad = false;
                    cx.notify();
                }
                _ => {}
            }
        }));
        let watch = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            loop {
                if this.update(cx, |this, cx| this.check_applications(cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(EVERY).await;
            }
        });
        (
            Home {
                search,
                invite_field,
                invite_bad: false,
                shakes: 0,
                shown: None,
                browse: HashMap::new(),
                joining: HashSet::new(),
                joined: HashSet::new(),
                errors: HashMap::new(),
                withdrawing: HashSet::new(),
                sso_waiting: None,
                sso_done: HashSet::new(),
                known: HashMap::new(),
                invite: None,
                apply: None,
                create,
                _watch: watch,
            },
            subs,
        )
    }

    /// The window moved somewhere: Browse is asked again next time it shows,
    /// and an invite's page closes, unless it's waiting for you to sign in
    /// to its instance.
    pub fn moved(&mut self, nav: &Nav) {
        self.shown = None;
        if let Some(page) = &self.invite {
            let stays = page.signing_in && matches!(nav, Nav::Instance { key } if *key == page.key);
            if !stays {
                self.invite = None;
            }
        }
    }
}

/// "key|server", how joins and errors are kept.
pub(crate) fn tag(key: &str, server: &str) -> String {
    format!("{key}|{server}")
}

impl FuwaApp {
    // ───────────────────────── Acting ─────────────────────────

    fn load_browse(&mut self, key: &str, cx: &mut Context<Self>) {
        self.home.browse.entry(key.to_owned()).or_insert(Browse::Loading);
        let (core, k) = (self.core.clone(), key.to_owned());
        self.run(cx, async move { core.discover(&k).await }, {
            let key = key.to_owned();
            move |this, result, cx| {
                let browse = match result {
                    Ok(list) => {
                        for s in &list {
                            this.home.known.insert(tag(&key, &s.id), s.clone());
                        }
                        Browse::Ready(list)
                    }
                    Err(err) => Browse::Failed(err.message),
                };
                this.home.browse.insert(key, browse);
                cx.notify();
            }
        });
    }

    /// "Open" in the invite box: the invite's page, on its own instance.
    pub(crate) fn open_typed_invite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Nav::Instance { key } = self.nav.clone() else { return };
        let text = self.home.invite_field.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some((at, code)) = join::parse_invite(&text, &key) else {
            self.home.invite_bad = true;
            self.home.shakes += 1;
            cx.notify();
            return;
        };
        self.home.invite_field.update(cx, |s, cx| s.set_value("", window, cx));
        self.open_invite_page(&key, &at, &code, window, cx);
    }

    /// Joins a server from Browse or an invite; a tick, then it opens.
    pub(crate) fn join_now(
        &mut self,
        key: &str,
        server: &pb::Server,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = tag(key, &server.id);
        if !self.home.joining.insert(id.clone()) {
            return;
        }
        self.home.errors.remove(&id);
        cx.notify();
        let (core, k, sid, c) = (self.core.clone(), key.to_owned(), server.id.clone(), code.to_owned());
        let channel = self
            .home
            .invite
            .as_ref()
            .and_then(|p| p.found.as_ref()?.as_ref().ok())
            .filter(|f| f.server.id == server.id && !f.channel_name.is_empty())
            .map(|f| f.invite.channel_id.clone());
        self.run_in(window, cx, async move { core.join_server(&k, &sid, &c).await }, {
            let key = key.to_owned();
            move |this, result, window, cx| {
                this.home.joining.remove(&id);
                match result {
                    Ok(server) => {
                        this.home.joined.insert(id.clone());
                        cx.spawn_in(window, async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(650)).await;
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.home.joined.remove(&id);
                                this.open_joined(&key, &server.id, channel.as_deref(), window, cx);
                            });
                        })
                        .detach();
                    }
                    Err(err) => {
                        this.home.errors.insert(id, err.message);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Goes to a server you're in, at an invite's channel when it has one.
    pub(crate) fn open_joined(
        &mut self,
        key: &str,
        server: &str,
        channel: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.home.invite = None;
        match channel {
            Some(ch) => self.open_channel(key, server, ch, window, cx),
            None => self.navigate(Nav::Server { key: key.to_owned(), server: server.to_owned() }, window, cx),
        }
        cx.notify();
    }

    /// Runs `future` on the core, then `done` with the window.
    pub(crate) fn run_in<T: Send + 'static>(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let rx = self.core.spawn(future);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(value) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| done(this, value, window, cx));
            }
        })
        .detach();
    }

    pub(crate) fn withdraw(&mut self, key: &str, server: &pb::Server, cx: &mut Context<Self>) {
        let id = tag(key, &server.id);
        if !self.home.withdrawing.insert(id.clone()) {
            return;
        }
        let (core, k, sid, name) = (self.core.clone(), key.to_owned(), server.id.clone(), server.name.clone());
        self.run(cx, async move { core.withdraw_application(&k, &sid).await }, move |this, result, cx| {
            this.home.withdrawing.remove(&id);
            match result {
                Ok(()) => this.toast(
                    "undo-2",
                    t_with("join.withdrawn", &[("server", Arg::Str(&name))]),
                    String::new(),
                    None,
                    None,
                    cx,
                ),
                Err(err) => {
                    this.home.errors.insert(id, err.message);
                }
            }
            cx.notify();
        });
    }

    /// Signs in through a server's provider, then joins (or opens the application).
    pub(crate) fn sso_join(
        &mut self,
        key: &str,
        server: &pb::Server,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = tag(key, &server.id);
        self.home.sso_waiting = Some(id.clone());
        self.home.errors.remove(&id);
        cx.notify();
        let (core, k, sid, c) = (self.core.clone(), key.to_owned(), server.id.clone(), code.to_owned());
        let server = server.clone();
        self.run_in(window, cx, async move { core.server_sso(&k, &sid, Some(c), crate::ui::open_in_browser).await }, {
            let (key, code) = (key.to_owned(), code.to_owned());
            move |this, result, window, cx| {
                if this.home.sso_waiting.as_deref() != Some(id.as_str()) {
                    return;
                }
                this.home.sso_waiting = None;
                match result {
                    // Already a member: back in.
                    Ok(Some(_)) => this.open_joined(&key, &server.id, None, window, cx),
                    Ok(None) => {
                        this.home.sso_done.insert(id);
                        if server.applications {
                            this.open_apply(&key, &server, &code, window, cx);
                        } else {
                            this.join_now(&key, &server, &code, window, cx);
                        }
                    }
                    Err(err) => {
                        this.home.errors.insert(id, err.message);
                    }
                }
                cx.notify();
            }
        });
    }

    /// Asks each instance how your applications went, and says so when one
    /// lets you in or turns you down. Nothing is asked while you wait on none.
    fn check_applications(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.core.shared.read(|s| {
            s.order
                .iter()
                .filter_map(|k| s.instance(k))
                .filter(|i| i.me.is_some() && i.connection == Connection::Live)
                .map(|i| i.key.clone())
                .collect()
        });
        for key in keys {
            self.core.load_applied(&key);
            let waiting = self.core.shared.read(|s| {
                s.instance(&key).and_then(|i| i.applied.as_ref()).is_some_and(|a| a.values().any(|a| a.waiting()))
            });
            if !waiting {
                continue;
            }
            let (core, k) = (self.core.clone(), key.clone());
            self.run(cx, async move { core.check_applied(&k).await }, move |this, checked, cx| {
                for s in checked.let_in {
                    this.toast(
                        "party-popper",
                        t_with("join.applied.letIn", &[("server", Arg::Str(&s.name))]),
                        String::new(),
                        Some(Nav::Server { key: key.clone(), server: s.id.clone() }),
                        None,
                        cx,
                    );
                }
                for s in checked.turned_down {
                    this.toast(
                        "x",
                        t_with("join.applied.turnedDown", &[("server", Arg::Str(&s.name))]),
                        String::new(),
                        None,
                        None,
                        cx,
                    );
                }
                cx.notify();
            });
        }
    }

    // ───────────────────────── The page ─────────────────────────

    pub(crate) fn instance_home(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some((connection, me, problem)) =
            self.core.shared.read(|s| s.instance(key).map(|i| (i.connection, i.me.is_some(), i.problem.clone())))
        else {
            return div().into_any_element();
        };
        if self.home.invite.as_ref().is_some_and(|page| page.key == key || page.on == key) {
            return self.invite_page(window, cx);
        }
        let signed_out = connection == Connection::SignedOut || (!me && connection != Connection::Connecting);
        if signed_out {
            return self.signed_out_page(key, problem, &p, cx);
        }
        if self.home.shown.as_deref() != Some(key) && me {
            self.home.shown = Some(key.to_owned());
            self.load_browse(key, cx);
        }
        let viewport = window.viewport_size();
        let vw = f32::from(viewport.width);
        let main = (vw - BESIDE).max(320.0);
        let pad = if vw >= 640.0 { 32.0 } else { 16.0 };
        let width = main.min(1024.0) - pad * 2.0;
        let cols = if vw >= 1024.0 {
            3
        } else if vw >= 640.0 {
            2
        } else {
            1
        };
        let body = div()
            .w(px(width))
            .flex()
            .flex_col()
            .child(self.hero(key, width, &p, window, cx))
            .child(self.have_invite(&p, window, cx))
            .child(self.browse_header(&p, window, cx))
            .child(self.browse_results(key, width, cols, &p, window, cx));
        div()
            .id("instance-page")
            .size_full()
            .overflow_y_scroll()
            .child(div().w_full().flex().justify_center().pt(px(24.0)).pb(px(64.0)).child(body))
            .into_any_element()
    }

    /// The instance's welcome: how it's connected, its name, and making a server.
    fn hero(&mut self, key: &str, width: f32, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (name, connection, node, url) = self
            .core
            .shared
            .read(|s| s.instance(key).map(|i| (i.name(), i.connection, i.node.clone(), i.url.clone())))
            .unwrap_or_default_tuple();
        let streamer = self.prefs.streamer_mode;
        let label = connection_label(connection);
        let build = node.as_ref().map(|n| {
            let commit: String = n.build.as_ref().map(|b| b.commit.chars().take(7).collect()).unwrap_or_default();
            (format!("fuwa {}", n.version), commit)
        });
        let muted = p.muted_foreground;
        let status = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.0))
            .gap_y(px(6.0))
            .min_h(px(21.0))
            .text_size(px(14.0))
            .line_height(px(20.0))
            .font_weight(FontWeight::BOLD)
            .text_color(muted)
            .child(conn_dot(connection, p, window))
            .child(format!("{label} ·"))
            .child(if streamer { "•••••".to_owned() } else { key.replace('~', "/") })
            .child("·")
            .when_some(build, |el, (version, commit)| {
                el.child(div().flex().items_center().child(version).when(!commit.is_empty(), |el| {
                    el.child(div().font_family("monospace").font_weight(FontWeight::BOLD).child(format!(" · {commit}")))
                }))
            })
            .when(hosted_by_us(&url), |el| el.child(hosted_chip(p)));
        let welcome = template_text("workspace.home.welcome");
        let (before, after) = welcome.split_once("{name}").unwrap_or((welcome.as_str(), ""));
        let title = div()
            .mt(px(8.0))
            .text_size(px(36.0))
            .line_height(px(40.0))
            .font_weight(FontWeight::EXTRA_BOLD)
            .child(gradient_text(before, &name, after, p));
        let create = hero_button("home-create", "plus", &t("workspace.home.create"), p).on_click(cx.listener({
            let k = key.to_owned();
            move |this, _, window, cx| this.open_dialog(Dialog::CreateServer { key: k.clone() }, window, cx)
        }));
        let row = div()
            .relative()
            .flex()
            .items_end()
            .justify_between()
            .gap(px(16.0))
            .child(
                div().flex_1().min_w_0().max_w(px(576.0)).flex().flex_col().child(status).child(title).child(
                    div()
                        .mt(px(8.0))
                        .text_size(px(16.0))
                        .line_height(px(24.0))
                        .text_color(muted)
                        .child(t("workspace.home.about")),
                ),
            )
            .child(div().flex_none().child(create));
        let inner = width - 2.0;
        div()
            .relative()
            .w_full()
            .overflow_hidden()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(40.0))
            .child(dot_grid(0.6, p.foreground))
            // The web's blurred primary circle at the top right (`size-64 blur-3xl`).
            .child(soft_glow(inner - 256.0 + 64.0, -80.0, 256.0, 256.0, alpha(p.primary, 0.25), 64.0))
            .child(row)
            .into_any_element()
    }

    /// Servers out of Browse are joined by invite: paste a link (from any instance) or a code.
    fn have_invite(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let bad = self.home.invite_bad;
        let typed = !self.home.invite_field.read(cx).value().trim().is_empty();
        let streamer = self.prefs.streamer_mode;
        let field = div()
            .flex_1()
            .min_w_0()
            .h(px(40.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .rounded(radius_xl())
            .border_1()
            .border_color(if bad { p.destructive } else { p.border })
            .text_size(px(14.0))
            .when(streamer, |el| el.opacity(0.85))
            .child(div().flex_1().min_w_0().child(Input::new(&self.home.invite_field).appearance(false).small()));
        let field = focus_ring(field, has_focus(&self.home.invite_field, window, cx), p);
        let open = div()
            .id("home-invite-open")
            .h(px(40.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(radius_xl())
            .bg(p.primary)
            .text_color(p.primary_foreground)
            .text_size(px(14.0))
            .font_weight(FontWeight::BOLD)
            .when(!typed, |el| el.opacity(0.5))
            .when(typed, |el| {
                let c = mix(p.primary, p.background, 0.1);
                el.cursor_pointer().hover(move |s| s.bg(c))
            })
            .child(t("workspace.home.invite.open"))
            .child(icon("arrow-right").size(px(16.0)))
            .on_click(cx.listener(|this, _, window, cx| this.open_typed_invite(window, cx)));
        let shakes = self.home.shakes;
        let line = div().flex_1().flex().gap(px(8.0)).child(field).child(open);
        let line = if shakes > 0 {
            motion::once(
                line,
                SharedString::from(format!("invite-shake-{shakes}")),
                Duration::from_millis(400),
                |el, t| el.relative().left(px(shake(t))),
            )
        } else {
            line.into_any_element()
        };
        div()
            .mt(px(16.0))
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(alpha(p.card, 0.7))
            .child(
                div()
                    .w(px(256.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .size(px(40.0))
                            .flex_none()
                            .rounded(radius_2xl())
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(alpha(p.primary, 0.15))
                            .text_color(p.primary)
                            .child(icon("ticket").size(px(20.0)).rotate(gpui_kit::radians(-12f32.to_radians()))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .line_height(px(24.0))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(t("workspace.home.invite.title")),
                            )
                            .child(
                                div().text_size(px(12.0)).line_height(px(16.0)).text_color(p.muted_foreground).child(
                                    if bad { t("workspace.home.invite.bad") } else { t("workspace.home.invite.hint") },
                                ),
                            ),
                    ),
            )
            .child(line)
            .into_any_element()
    }

    fn browse_header(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = has_focus(&self.home.search, window, cx);
        div()
            .mt(px(32.0))
            .mb(px(16.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(18.0))
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(t("workspace.home.browse")),
            )
            .child(focus_ring(
                div()
                    .w(px(288.0))
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .pl(px(12.0))
                    .pr(px(12.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .text_size(px(14.0))
                    .child(icon("search").size(px(16.0)).text_color(p.muted_foreground))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.home.search).appearance(false).small())),
                focused,
                p,
            ))
            .into_any_element()
    }

    /// The servers in Browse that match the search, rising in one after another.
    fn browse_results(
        &mut self,
        key: &str,
        width: f32,
        cols: u16,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let grid = || div().flex().gap(px(16.0));
        let servers = match self.home.browse.get(key).cloned() {
            None | Some(Browse::Loading) => {
                return grid()
                    .children(
                        (0..usize::from(cols))
                            .map(|n| div().flex_1().child(shimmer(176.0, radius_3xl(), p, window, n))),
                    )
                    .into_any_element();
            }
            Some(Browse::Failed(error)) => {
                return div().text_sm().text_color(p.muted_foreground).child(capitalized(&error)).into_any_element();
            }
            Some(Browse::Ready(list)) => list,
        };
        let query = self.home.search.read(cx).value().trim().to_lowercase();
        let shown: Vec<pb::Server> = servers
            .into_iter()
            .filter(|s| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
            })
            .collect();
        if shown.is_empty() {
            let searching = !query.is_empty();
            return div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .p(px(40.0))
                .rounded(radius_3xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .text_center()
                .child(div().font_weight(FontWeight::BOLD).child(if searching {
                    t("workspace.home.noMatch")
                } else {
                    t("workspace.home.noServers")
                }))
                .child(div().mt(px(4.0)).text_sm().text_color(p.muted_foreground).child(if searching {
                    t("workspace.home.tryAnother")
                } else {
                    t("workspace.home.makeFirst")
                }))
                .into_any_element();
        }
        let card_w = (width - 16.0 * f32::from(cols - 1)) / f32::from(cols);
        let mut rows = div().flex().flex_col().gap(px(16.0));
        for (r, chunk) in shown.chunks(usize::from(cols)).enumerate() {
            let mut row = div().flex().gap(px(16.0));
            for (c, s) in chunk.iter().enumerate() {
                let n = r * usize::from(cols) + c;
                let card = self.server_card(key, s, card_w, p, window, cx);
                row = row.child(motion::rise(
                    div().w(px(card_w)).flex_none().flex().flex_col().child(card),
                    SharedString::from(format!("browse|{key}|{}", s.id)),
                    Duration::from_millis(50 * n.min(8) as u64),
                    20.0,
                ));
            }
            rows = rows.child(row);
        }
        rows.into_any_element()
    }

    /// A server in Browse: its banner, icon, name, how many are in it, what
    /// it's about, what it asks of people coming in, and the way in.
    fn server_card(
        &mut self,
        key: &str,
        server: &pb::Server,
        w: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let banner = crate::ui::banner::server_banner(server, w - 2.0, 80.0, Some(p.card.into()), window, cx);
        let members = server.member_count;
        let border = p.border;
        let hover = p.primary;
        let glow = alpha(p.primary, 0.55);
        let r = f32::from(radius_3xl());
        div()
            .id(SharedString::from(format!("card|{key}|{}", server.id)))
            .relative()
            .h_full()
            .flex()
            .flex_col()
            .rounded(radius_3xl())
            .border_1()
            .border_color(border)
            .bg(p.card)
            .hover(move |s| {
                s.border_color(hover).translate_y(px(-4.0)).shadow(vec![BoxShadow {
                    color: glow,
                    offset: point(px(0.0), px(18.0)),
                    blur_radius: px(20.0),
                    spread_radius: px(-22.0),
                    inset: false,
                }])
            })
            // GPUI counts a negative margin into a column's height, so the
            // banner hangs out of a 44px slot instead of the rest moving up.
            .child(
                div().relative().w_full().h(px(45.0)).flex_none().child(
                    div()
                        .absolute()
                        .top(px(1.0))
                        .left(px(1.0))
                        .w(px(w - 2.0))
                        .h(px(80.0))
                        .overflow_hidden()
                        .rounded_t(px(r - 1.0))
                        .child(banner),
                ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .px(px(20.0))
                    .pb(px(20.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div().flex().items_end().gap(px(12.0)).child(ringed_icon(server, 56.0, 18.0, 4.0, p)).child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(18.0))
                                        .line_height(px(28.0))
                                        .font_weight(FontWeight::EXTRA_BOLD)
                                        .child(server.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(p.muted_foreground)
                                        .child(icon("users").size(px(14.0)))
                                        .child(t_with("workspace.home.members", &[("count", Arg::Num(members))])),
                                ),
                        ),
                    )
                    .child(
                        div()
                            .w(px(w - 42.0))
                            .flex_1()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .overflow_hidden()
                            .child(if server.description.trim().is_empty() {
                                StyledText::new(t("workspace.home.noDescription")).into_any_element()
                            } else {
                                inline_markdown(&server.description, p.foreground).into_any_element()
                            }),
                    )
                    .children(server_door(server, p))
                    .child(self.join_button(key, server, "", false, None, p, window, cx)),
            )
            .into_any_element()
    }

    /// The one way into a server, as Browse cards and invite pages show it:
    /// Open, Join, Apply, where your application stands, a provider's
    /// sign-in, or why you can't. `large` is the invite page's.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn join_button(
        &mut self,
        key: &str,
        server: &pb::Server,
        code: &str,
        large: bool,
        open_label: Option<String>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = tag(key, &server.id);
        let joined = self.home.joined.contains(&id);
        let signed_in = self.home.sso_done.contains(&id);
        self.home.known.insert(id.clone(), server.clone());
        let (kind, applied, my_kind) = self
            .core
            .shared
            .read(|s| {
                s.instance(key).map(|i| {
                    (
                        join::kind_for(i, server, joined, signed_in),
                        i.applied.as_ref().and_then(|a| a.get(&server.id)).cloned(),
                        i.me.as_ref().map(|m| m.kind).unwrap_or_default(),
                    )
                })
            })
            .unwrap_or((Kind::Join, None, 0));
        let tall = if large { 44.0 } else { 40.0 };
        let pending = self.home.joining.contains(&id);
        let name = server.name.clone();
        let (button, note): (AnyElement, Option<AnyElement>) = match kind {
            Kind::Open => {
                let label = open_label.unwrap_or_else(|| t("join.button.open"));
                let (k, sid) = (key.to_owned(), server.id.clone());
                (
                    outline_button(SharedString::from(format!("open|{id}")), tall, p)
                        .child(label)
                        .child(arrow_nudge(&id, p))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_joined(&k, &sid, None, window, cx)))
                        .into_any_element(),
                    None,
                )
            }
            Kind::LinkedOnly => (
                outline_button(SharedString::from(format!("linked|{id}")), tall, p)
                    .opacity(0.5)
                    .cursor_default()
                    .child(icon("lock").size(px(16.0)))
                    .child(t("join.button.linkedOnly"))
                    .into_any_element(),
                Some(
                    note_text(
                        &t(if my_kind == pb::AccountKind::Sso as i32 {
                            "join.button.linkedOnlySso"
                        } else {
                            "join.button.linkedOnlyLocal"
                        }),
                        p,
                    )
                    .into_any_element(),
                ),
            ),
            Kind::Sso => {
                let provider = if server.sso_name.is_empty() {
                    t("connect.provider.yourOrganization")
                } else {
                    server.sso_name.clone()
                };
                let waiting = self.home.sso_waiting.as_deref() == Some(id.as_str());
                let label = if waiting {
                    t_with("connect.provider.leaving", &[("name", Arg::Str(&provider))])
                } else if server.applications {
                    t("join.button.signInToApply")
                } else {
                    t_with("join.button.joinWith", &[("name", Arg::Str(&provider))])
                };
                let (k, s, c) = (key.to_owned(), server.clone(), code.to_owned());
                let note = if server.sso_host.is_empty() {
                    t_with("join.button.ssoNote", &[("server", Arg::Str(&server.name)), ("name", Arg::Str(&provider))])
                } else {
                    t_with(
                        "join.button.ssoNoteAt",
                        &[
                            ("server", Arg::Str(&server.name)),
                            ("name", Arg::Str(&provider)),
                            ("host", Arg::Str(&server.sso_host)),
                        ],
                    )
                };
                (
                    provider_button(SharedString::from(format!("sso|{id}")), &label, waiting, p, window)
                        .on_click(cx.listener(move |this, _, window, cx| this.sso_join(&k, &s, &c, window, cx)))
                        .into_any_element(),
                    Some(note_text(&note, p).into_any_element()),
                )
            }
            Kind::Waiting => {
                let amber: Hsla = gpui_kit::rgb(0xd97706).into();
                let badge = div()
                    .w_full()
                    .h(px(tall))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .rounded(radius_xl())
                    .bg(alpha(gpui_kit::rgb(0xf59e0b), 0.15))
                    .text_color(amber)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .child(hourglass(16.0, &id, window))
                    .child(t("join.button.waiting"));
                let (k1, s1) = (key.to_owned(), server.id.clone());
                let (k2, s2) = (key.to_owned(), server.clone());
                let template = template_text("join.button.waitingNote");
                let look = link_text(SharedString::from(format!("look|{id}")), &t("join.seeWhereItStands"), p)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_dialog(Dialog::Application { key: k1.clone(), server: s1.clone() }, window, cx)
                    }));
                let withdraw = link_text(SharedString::from(format!("withdraw|{id}")), &t("join.button.takeItBack"), p)
                    .on_click(cx.listener(move |this, _, _, cx| this.withdraw(&k2, &s2, cx)));
                (badge.into_any_element(), Some(sentence_with(&template, look, withdraw, p).into_any_element()))
            }
            Kind::Apply => {
                let again = applied.as_ref().is_some_and(|a| a.status == pb::ApplicationStatus::Rejected);
                let (k, s, c) = (key.to_owned(), server.clone(), code.to_owned());
                let note = applied.filter(|_| again).map(|a| {
                    note_text(
                        &if a.reason.is_empty() {
                            t("join.button.lastTurnedDown")
                        } else {
                            t_with("join.button.lastTurnedDownBecause", &[("reason", Arg::Str(&a.reason))])
                        },
                        p,
                    )
                    .into_any_element()
                });
                (
                    filled_button(SharedString::from(format!("apply|{id}")), tall, p.primary, p.primary_foreground, p)
                        .child(icon("clipboard-pen").size(px(16.0)))
                        .child(if again { t("join.applyAgain") } else { t("join.apply") })
                        .on_click(cx.listener(move |this, _, window, cx| this.open_apply(&k, &s, &c, window, cx)))
                        .into_any_element(),
                    note,
                )
            }
            Kind::Join | Kind::Joined => {
                let done = kind == Kind::Joined;
                let green = gpui_kit::rgb(0x10b981);
                let label = if done {
                    t("join.button.joined")
                } else if large {
                    t_with("join.button.joinServer", &[("server", Arg::Str(&name))])
                } else {
                    t("join.button.join")
                };
                let (k, s, c) = (key.to_owned(), server.clone(), code.to_owned());
                let state = if done {
                    "done"
                } else if pending {
                    "busy"
                } else {
                    "join"
                };
                let inner = div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(done, |el| el.child(icon("check").size(px(16.0))))
                    .when(pending && !done, |el| el.child(spinner(16.0, &id, window)))
                    .child(label);
                (
                    filled_button(
                        SharedString::from(format!("join|{id}")),
                        tall,
                        if done { green } else { p.primary },
                        if done { gpui_kit::rgb(0xffffff) } else { p.primary_foreground },
                        p,
                    )
                    .when(pending, |el| el.opacity(0.5))
                    .child(motion::rise(
                        inner,
                        SharedString::from(format!("join-state|{id}|{state}")),
                        Duration::ZERO,
                        8.0,
                    ))
                    .when(!pending && !done, |el| {
                        el.on_click(cx.listener(move |this, _, window, cx| this.join_now(&k, &s, &c, window, cx)))
                    })
                    .into_any_element(),
                    None,
                )
            }
        };
        let error = self.home.errors.get(&id).cloned();
        let kind_tag = match kind {
            Kind::Joined | Kind::Join => "join",
            Kind::Open => "open",
            Kind::LinkedOnly => "linked",
            Kind::Waiting => "waiting",
            Kind::Sso => "sso",
            Kind::Apply => "apply",
        };
        let line: Option<AnyElement> = match error {
            Some(error) => Some(motion::once(
                div()
                    .w_full()
                    .text_center()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(p.destructive)
                    .child(capitalized(&error)),
                SharedString::from(format!("join-error|{id}|{error}")),
                Duration::from_millis(800),
                |el, t| el.relative().left(px(shake(t) * 0.75)),
            )),
            None => note,
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(motion::once(
                div().w_full().child(button),
                SharedString::from(format!("join-kind|{id}|{kind_tag}")),
                Duration::from_millis(260),
                |el, t| el.opacity(t.clamp(0.0, 1.0)),
            ))
            .children(line)
            .into_any_element()
    }

    /// Signed out of this instance: sign in again, where its sessions are kept.
    fn signed_out_page(
        &mut self,
        key: &str,
        problem: Option<String>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let address = if self.prefs.streamer_mode { "•••••".to_owned() } else { key.replace('~', "/") };
        let k = key.to_owned();
        let card = div()
            .w_full()
            .max_w(px(448.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .p(px(32.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(shadow_xl(p))
            .when_some(problem, |el, problem| {
                el.child(div().p(px(12.0)).rounded(radius_2xl()).bg(p.muted).text_sm().child(capitalized(&problem)))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t_with("workspace.home.connectTo", &[("address", Arg::Str(&address))])),
                    )
                    .child(div().text_sm().text_color(p.muted_foreground).child(t("workspace.home.notSignedIn"))),
            )
            .child(
                filled_button("signed-out-again", 44.0, p.primary, p.primary_foreground, p)
                    .child(icon("log-in").size(px(16.0)))
                    .child(t("connect.account.signIn"))
                    .on_click(cx.listener(move |this, _, window, cx| this.reconnect(&k, window, cx))),
            );
        div()
            .id("instance-page")
            .size_full()
            .overflow_y_scroll()
            .child(div().w_full().min_h_full().flex().items_center().justify_center().p(px(16.0)).child(motion::rise(
                card,
                SharedString::from(format!("signed-out|{key}")),
                Duration::ZERO,
                18.0,
            )))
            .into_any_element()
    }
}

/// A tuple of an instance's details, or empty ones.
trait OrEmpty {
    fn unwrap_or_default_tuple(self) -> (String, Connection, Option<pb::Node>, String);
}

impl OrEmpty for Option<(String, Connection, Option<pb::Node>, String)> {
    fn unwrap_or_default_tuple(self) -> (String, Connection, Option<pb::Node>, String) {
        self.unwrap_or_else(|| (String::new(), Connection::Connecting, None, String::new()))
    }
}

// ───────────────────────── Pieces ─────────────────────────

/// A line from the translations with its placeholders left in, for drawing
/// some of them bold (`text::hint_line`) or as links.
pub(crate) fn template_text(key: &str) -> String {
    t(key)
}

/// What a connection is doing, in words.
pub(crate) fn connection_label(c: Connection) -> String {
    t(match c {
        Connection::Live => "workspace.connection.live",
        Connection::Connecting => "workspace.connection.connecting",
        Connection::Reconnecting => "workspace.connection.reconnecting",
        Connection::Offline => "workspace.connection.offline",
        Connection::SignedOut => "workspace.connection.signedOut",
    })
}

/// The web's `.conn-dot` (0.55rem), pinging while live and breathing while it connects.
fn conn_dot(c: Connection, p: &Palette, window: &Window) -> AnyElement {
    let dot = crate::ui::widgets::conn_dot(c, p).size(px(8.8)).border_0();
    match c {
        Connection::Live => {
            let ring: Hsla = gpui_kit::rgb(0x3ecf8e).into();
            let ping = motion::ambient(
                div().absolute().rounded_full().bg(ring),
                "home-conn-ping",
                Duration::from_millis(2400),
                window,
                |el, t| {
                    let k = (t / 0.75).min(1.0);
                    let grow = 1.0 + k;
                    let s = 8.8 * grow;
                    el.size(px(s)).left(px((8.8 - s) / 2.0)).top(px((8.8 - s) / 2.0)).opacity(1.0 - k)
                },
            );
            div().relative().size(px(8.8)).flex_none().child(ping).child(dot.absolute()).into_any_element()
        }
        Connection::Connecting | Connection::Reconnecting => motion::ambient(
            div().flex_none().child(dot),
            "home-conn-breathe",
            Duration::from_millis(1200),
            window,
            |el, t| el.opacity(1.0 - 0.65 * (t * std::f32::consts::PI).sin()),
        ),
        _ => dot.flex_none().into_any_element(),
    }
}

/// Text on the web's `.gradient-text`: the primary sweeping toward violet,
/// one color per letter.
fn gradient_text(before: &str, name: &str, after: &str, p: &Palette) -> Tracked {
    let text = format!("{before}{name}{after}");
    let start: Hsla = p.primary.into();
    let violet = gpui_kit::rgb(0xa78bfa);
    let end = mix(p.primary, violet, 0.55);
    let n = name.chars().count().max(2) - 1;
    let plain: Hsla = p.foreground.into();
    let colors = before
        .chars()
        .map(|_| plain)
        .chain(name.chars().enumerate().map(|(i, _)| lerp(start, end, i as f32 / n as f32)))
        .chain(after.chars().map(|_| plain))
        .collect();
    tracked(text, TIGHT).letter_colors(colors)
}

fn lerp(a: Hsla, b: Hsla, f: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    Rgba { r: a.r + (b.r - a.r) * f, g: a.g + (b.g - a.g) * f, b: a.b + (b.b - a.b) * f, a: a.a + (b.a - a.a) * f }
        .into()
}

/// The web's `.dot-grid`: dots every 22px, faded out toward the edges.
pub(crate) fn dot_grid(opacity: f32, dot: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), window, _| {
            let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            // An ellipse to the farthest corner: opaque to 30% of the way, gone by 75%.
            let (rx, ry) = (w / 2.0 * std::f32::consts::SQRT_2, h / 2.0 * std::f32::consts::SQRT_2);
            let mut y = 11.0;
            while y < h {
                let mut x = 11.0;
                while x < w {
                    let d = (((x - w / 2.0) / rx).powi(2) + ((y - h / 2.0) / ry).powi(2)).sqrt();
                    let mask = ((0.75 - d) / 0.45).clamp(0.0, 1.0);
                    if mask > 0.0 {
                        let a = 0.12 * opacity * mask;
                        let b = Bounds::new(point(px(left + x - 1.0), px(top + y - 1.0)), size(px(2.0), px(2.0)));
                        window.paint_quad(fill(b, Rgba { a, ..dot }).corner_radii(px(1.0)));
                    }
                    x += 22.0;
                }
                y += 22.0;
            }
        },
    )
    .absolute()
    .inset_0()
}

/// A blurred round light (the web's `rounded-full blur-3xl` blobs), `sigma`
/// being the blur's spread. GPUI leaves an element's own area out of its
/// shadow, so the element sits far to the left and casts its shadow back.
pub(crate) fn soft_glow(left: f32, top: f32, w: f32, h: f32, color: Hsla, sigma: f32) -> Div {
    const AWAY: f32 = 4096.0;
    div().absolute().left(px(left - AWAY)).top(px(top)).w(px(w)).h(px(h)).rounded(px(w.min(h) / 2.0)).shadow(vec![
        BoxShadow {
            color,
            offset: point(px(AWAY), px(0.0)),
            blur_radius: px(sigma),
            spread_radius: px(0.0),
            inset: false,
        },
    ])
}

/// A field's focus, as the web's inputs show it: the ring color on the
/// border and a 3px ring at half strength around it.
pub(crate) fn focus_ring(el: Div, focused: bool, p: &Palette) -> Div {
    el.when(focused, |el| {
        // GPUI fills a shadow under the element too, so the field gets the card's color.
        el.border_color(p.primary).bg(p.card).shadow(vec![BoxShadow {
            color: alpha(p.primary, 0.5),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(3.0),
            inset: false,
        }])
    })
}

/// Whether a field has the keyboard.
pub(crate) fn has_focus<T: gpui_kit::Focusable>(state: &Entity<T>, window: &Window, cx: &App) -> bool {
    state.read(cx).focus_handle(cx).is_focused(window)
}

/// "Hosted by Waifu Devs", for fuwa.chat over https alone.
pub(crate) fn hosted_by_us(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or_default();
    u.scheme() == "https" && (host == "fuwa.chat" || host.ends_with(".fuwa.chat"))
}

fn hosted_chip(p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(22.0))
        .pl(px(7.0))
        .pr(px(10.0))
        .rounded_full()
        .border_1()
        .border_color(alpha(p.primary, 0.35))
        .bg(alpha(p.primary, 0.12))
        .text_color(p.primary)
        .text_size(px(12.0))
        .font_weight(FontWeight::BOLD)
        .child(icon("flower").size(px(14.0)))
        .child(t("shell.hosted.label"))
}

/// A big filled button on the hero (`size="lg"`, `h-11 rounded-xl`), lifting on hover like `.btn`.
fn hero_button(id: &str, glyph: &str, label: &str, p: &Palette) -> Stateful<Div> {
    filled_button(SharedString::from(id.to_owned()), 44.0, p.primary, p.primary_foreground, p)
        .px(px(16.0))
        .child(icon(glyph).size(px(16.0)))
        .child(label.to_owned())
}

/// The web's filled `Button` with `.btn`: a lift and a glow on hover, a dip on press.
pub(crate) fn filled_button(
    id: impl Into<gpui_kit::ElementId>,
    tall: f32,
    bg: Rgba,
    fg: Rgba,
    p: &Palette,
) -> Stateful<Div> {
    let glow = alpha(bg, 1.0);
    let _ = p;
    div()
        .id(id)
        .relative()
        .w_full()
        .h(px(tall))
        .px(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .rounded(radius_xl())
        .bg(bg)
        .text_color(fg)
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(move |s| {
            s.translate_y(px(-2.0)).shadow(vec![BoxShadow {
                color: glow,
                offset: point(px(0.0), px(8.0)),
                blur_radius: px(11.0),
                spread_radius: px(-8.0),
                inset: false,
            }])
        })
        .active(|s| s.translate_y(px(0.0)).scale(0.94))
}

/// The web's `variant="outline"` button: the page's color, a border, a soft shadow.
pub(crate) fn outline_button(id: impl Into<gpui_kit::ElementId>, tall: f32, p: &Palette) -> Stateful<Div> {
    let hover = p.accent;
    div()
        .id(id)
        .group("outline")
        .w_full()
        .h(px(tall))
        .px(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        .bg(p.background)
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.05),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(1.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
}

/// The arrow on "Open", nudging along on hover.
fn arrow_nudge(id: &str, _p: &Palette) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("nudge|{id}")))
        .group_hover("outline", |s| s.translate_x(px(4.0)))
        .child(icon("arrow-right").size(px(16.0)))
}

/// Signing in through a provider (the web's `ProviderButton`): dark, with
/// the provider's building in the primary color, spinning while it's away.
pub(crate) fn provider_button(
    id: impl Into<gpui_kit::ElementId>,
    label: &str,
    waiting: bool,
    p: &Palette,
    window: &Window,
) -> Stateful<Div> {
    let building = || div().text_color(p.primary).child(icon("building").size(px(20.0)));
    let glyph = if waiting {
        motion::ambient(building(), "provider-spin", Duration::from_millis(1200), window, |el, t| {
            el.rotate(gpui_kit::radians(t * std::f32::consts::TAU))
        })
    } else {
        // `group-hover:-translate-y-0.5 group-hover:scale-110`.
        building()
            .id("provider-building")
            .group_hover("provider", |s| s.translate_y(px(-2.0)).scale(1.1))
            .into_any_element()
    };
    div()
        .id(id)
        .group("provider")
        .relative()
        .w_full()
        .h(px(48.0))
        .px(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .rounded(radius_xl())
        .bg(p.foreground)
        .text_color(p.background)
        .font_weight(FontWeight::EXTRA_BOLD)
        .shadow(vec![BoxShadow {
            color: alpha(p.primary, 1.0),
            offset: point(px(0.0), px(14.0)),
            blur_radius: px(15.0),
            spread_radius: px(-16.0),
            inset: false,
        }])
        .when(waiting, |el| el.opacity(0.85))
        // `whileHover={{ y: -2 }} whileTap={{ scale: 0.97 }}`.
        .when(!waiting, |el| el.cursor_pointer().hover(|s| s.translate_y(px(-2.0))).active(|s| s.scale(0.97)))
        .child(glyph)
        .child(div().min_w_0().truncate().child(label.to_owned()))
        // The arrow nudges along on hover, and slips away while the browser's out.
        .child(
            div()
                .id("provider-arrow")
                .when(waiting, |el| el.opacity(0.0).translate_x(px(8.0)))
                .when(!waiting, |el| el.group_hover("provider", |s| s.translate_x(px(4.0))))
                .child(icon("arrow-right").size(px(16.0))),
        )
}

/// The line under a join button.
fn note_text(text: &str, p: &Palette) -> Div {
    div()
        .w_full()
        .text_center()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(p.muted_foreground)
        .child(text.to_owned())
}

/// A bold link inside a note.
fn link_text(id: SharedString, text: &str, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .font_weight(FontWeight::BOLD)
        .text_color(p.foreground)
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(text.to_owned())
}

/// "… {look} or {withdraw}": a sentence with two links in it, wrapping as text.
fn sentence_with(template: &str, look: Stateful<Div>, withdraw: Stateful<Div>, p: &Palette) -> Div {
    let mut el = div()
        .w_full()
        .flex()
        .flex_wrap()
        .justify_center()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(p.muted_foreground);
    let mut rest = template;
    let mut links = [("look", Some(look)), ("withdraw", Some(withdraw))];
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        for word in rest[..open].split_inclusive(' ') {
            el = el.child(word.replace(' ', "\u{a0}"));
        }
        let name = &rest[open + 1..open + close];
        if let Some((_, link)) = links.iter_mut().find(|(n, _)| *n == name)
            && let Some(link) = link.take()
        {
            el = el.child(link);
        }
        rest = &rest[open + close + 1..];
    }
    for word in rest.split_inclusive(' ') {
        el = el.child(word.replace(' ', "\u{a0}"));
    }
    el
}

/// The hourglass of a waiting application, turning over every few seconds.
pub(crate) fn hourglass(size_px: f32, id: &str, window: &Window) -> AnyElement {
    motion::ambient(
        icon("hourglass").size(px(size_px)),
        SharedString::from(format!("hourglass|{id}")),
        Duration::from_secs(3),
        window,
        |el, t| {
            // Still, then a half turn over the middle of each beat.
            let k = ((t - 0.35) / 0.3).clamp(0.0, 1.0);
            let eased = k * k * (3.0 - 2.0 * k);
            el.rotate(gpui_kit::radians(eased * std::f32::consts::PI))
        },
    )
}

/// A spinner, the web's `LoaderCircleIcon animate-spin`.
pub(crate) fn spinner(size_px: f32, id: &str, window: &Window) -> AnyElement {
    motion::ambient(
        icon("loader-circle").size(px(size_px)),
        SharedString::from(format!("spin|{id}")),
        Duration::from_millis(1000),
        window,
        |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
    )
}

/// What stands between someone and a server, as little chips: applications,
/// rules, waifu.dev only, a minimum account age (the web's `ServerDoor`).
pub(crate) fn server_door(server: &pb::Server, p: &Palette) -> Option<AnyElement> {
    let mut chips: Vec<(&str, &str, String)> = Vec::new();
    if server.applications {
        chips.push(("apply", "clipboard-pen", t("join.apply")));
    }
    if server.has_rules {
        chips.push(("rules", "scroll-text", t("join.door.rules")));
    }
    if server.linked_only {
        chips.push(("linked", "badge-check", t("join.door.linkedOnly")));
    }
    if server.min_account_age_seconds > 0 {
        let age = crate::ui::join::duration_words(i64::from(server.min_account_age_seconds));
        chips.push(("age", "hourglass", t_with("join.door.accountAge", &[("age", Arg::Str(&age))])));
    }
    if chips.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(chips.into_iter().enumerate().map(|(n, (id, glyph, label))| {
                let apply = id == "apply";
                motion::rise(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded_full()
                        .bg(if apply { alpha(p.primary, 0.15) } else { p.muted.into() })
                        .text_color(if apply { p.primary } else { p.muted_foreground })
                        .text_size(px(11.2))
                        .line_height(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .child(icon(glyph).size(px(12.0)))
                        .child(label),
                    SharedString::from(format!("door|{}|{id}", server.id)),
                    Duration::from_millis(40 * n as u64),
                    4.0,
                )
            }))
            .into_any_element(),
    )
}

/// A server's icon with a ring in the card's color around it (`ring-4 ring-card`).
pub(crate) fn ringed_icon(server: &pb::Server, size_px: f32, radius: f32, ring: f32, p: &Palette) -> Div {
    div()
        .flex_none()
        .size(px(size_px + ring * 2.0))
        .m(px(-ring))
        .p(px(ring))
        .rounded(px(radius + ring))
        .bg(p.card)
        .child(server_icon(server, size_px, radius, p).text_size(px(size_px * 0.32)))
}

/// The web's `.shimmer`: a muted block with a light sweeping across it.
pub(crate) fn shimmer(h: f32, radius: Pixels, p: &Palette, window: &Window, n: usize) -> AnyElement {
    let (muted, card) = (p.muted, p.card);
    let light = mix(muted, card, 0.5);
    motion::ambient(
        div().h(px(h)).w_full().rounded(radius).overflow_hidden().bg(muted),
        SharedString::from(format!("shimmer-{n}-{h}")),
        Duration::from_millis(1400),
        window,
        move |el, t| {
            el.bg(gpui_kit::linear_gradient(
                90.0,
                gpui_kit::linear_color_stop(muted, (t * 2.0 - 1.0).clamp(0.0, 1.0)),
                gpui_kit::linear_color_stop(light, (t * 2.0).clamp(0.0, 1.0)),
            ))
        },
    )
}

/// A left-and-right shake over `t` from 0 to 1, as the web's `.shake`.
pub(crate) fn shake(t: f32) -> f32 {
    let keys = [0.0, -6.0, 6.0, -3.0, 3.0, 0.0];
    let at = t.clamp(0.0, 1.0) * (keys.len() - 1) as f32;
    let i = (at.floor() as usize).min(keys.len() - 2);
    let f = at - i as f32;
    keys[i] + (keys[i + 1] - keys[i]) * f
}

/// "the server said no" → "The server said no" (the web's `first-letter:uppercase`).
pub(crate) fn capitalized(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The web's `shadow-xl`.
pub(crate) fn shadow_xl(_p: &Palette) -> Vec<BoxShadow> {
    vec![
        BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(20.0)),
            blur_radius: px(12.5),
            spread_radius: px(-5.0),
            inset: false,
        },
        BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.1),
            offset: point(px(0.0), px(8.0)),
            blur_radius: px(5.0),
            spread_radius: px(-6.0),
            inset: false,
        },
    ]
}

/// A line of Markdown as the web's `InlineMarkdown` draws it in descriptions
/// and rules: **bold** in `strong` (the text color), *italic*, `code`.
pub(crate) fn inline_markdown(source: &str, strong: Rgba) -> StyledText {
    let (text, runs) = inline_runs(source);
    let runs = runs
        .into_iter()
        .map(|(range, kind)| {
            let style = match kind {
                Mark::Bold => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    color: Some(strong.into()),
                    ..Default::default()
                },
                Mark::Italic => HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
                Mark::Code => HighlightStyle { color: Some(strong.into()), ..Default::default() },
            };
            (range, style)
        })
        .collect::<Vec<_>>();
    StyledText::new(text).with_highlights(runs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Bold,
    Italic,
    Code,
}

/// The text without its marks, and where each styled run is.
fn inline_runs(source: &str) -> (String, Vec<(std::ops::Range<usize>, Mark)>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut rest = source;
    while !rest.is_empty() {
        let found = [("**", Mark::Bold), ("__", Mark::Bold), ("`", Mark::Code), ("*", Mark::Italic)]
            .iter()
            .filter_map(|(mark, kind)| {
                let at = rest.find(mark)?;
                let close = rest[at + mark.len()..].find(mark)? + at + mark.len();
                (close > at + mark.len()).then_some((at, close, *mark, *kind))
            })
            .min_by_key(|(at, _, mark, _)| (*at, std::cmp::Reverse(mark.len())));
        let Some((at, close, mark, kind)) = found else {
            text.push_str(rest);
            break;
        };
        text.push_str(&rest[..at]);
        let start = text.len();
        text.push_str(&rest[at + mark.len()..close]);
        runs.push((start..text.len(), kind));
        rest = &rest[close + mark.len()..];
    }
    (text, runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_marks_become_styles() {
        let (text, runs) = inline_runs("No spoilers outside **#spoilers**.");
        assert_eq!(text, "No spoilers outside #spoilers.");
        assert_eq!(runs, vec![(20..29, Mark::Bold)]);
        let (text, runs) = inline_runs("a *b* `c` d");
        assert_eq!(text, "a b c d");
        assert_eq!(runs, vec![(2..3, Mark::Italic), (4..5, Mark::Code)]);
        assert_eq!(inline_runs("2 * 3 = 6").0, "2 * 3 = 6");
    }

    #[test]
    fn shakes_settle() {
        assert_eq!(shake(0.0), 0.0);
        assert_eq!(shake(1.0), 0.0);
        assert!(shake(0.2) < -5.0);
    }

    #[test]
    fn only_our_addresses_are_hosted() {
        assert!(hosted_by_us("https://fuwa.chat"));
        assert!(hosted_by_us("https://eu.fuwa.chat"));
        assert!(!hosted_by_us("http://fuwa.chat"));
        assert!(!hosted_by_us("https://notfuwa.chat"));
        assert!(!hosted_by_us("https://fuwa.chat.evil.com"));
    }

    #[test]
    fn first_letters_go_up() {
        assert_eq!(capitalized("that didn't work"), "That didn't work");
        assert_eq!(capitalized(""), "");
    }
}
