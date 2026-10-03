//! Connecting to an instance: its address, then signing in (with waifu.dev
//! in the browser, or a username and password kept on that instance), then
//! the two-step code if it's on.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Task, Window, div, px,
};

use crate::core::{Core, SignIn};
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{alpha, corner, mix};
use crate::ui::widgets::{card, error_line, fuwa_mark, icon, icon_button, labeled, pal, primary_button, soft_button};

pub enum ConnectEvent {
    Done { key: String },
    Cancel,
}

#[derive(Clone, PartialEq)]
enum Step {
    Address,
    Account,
    TwoFactor { ticket: String },
    Browser,
}

pub struct ConnectView {
    core: Arc<Core>,
    pub can_cancel: bool,
    step: Step,
    address: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    display_name: Entity<InputState>,
    code: Entity<InputState>,
    url: String,
    node: Option<pb::Node>,
    sign_up: bool,
    busy: bool,
    error: Option<String>,
    /// Who the browser is signing you in with, while it is.
    browser: String,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConnectEvent> for ConnectView {}

impl ConnectView {
    pub fn new(core: Arc<Core>, can_cancel: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("fuwa.chat"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("mika"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("••••••••"));
        let display_name = cx.new(|cx| InputState::new(window, cx).placeholder("Mika"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("123456"));
        let mut subs = Vec::new();
        for input in [&address, &username, &password, &display_name, &code] {
            subs.push(cx.subscribe_in(input, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            }));
        }
        address.update(cx, |s, cx| s.focus(window, cx));
        Self {
            core,
            can_cancel,
            step: Step::Address,
            address,
            username,
            password,
            display_name,
            code,
            url: String::new(),
            node: None,
            sign_up: false,
            busy: false,
            error: None,
            browser: String::new(),
            task: None,
            _subscriptions: subs,
        }
    }

    /// Starts at an address already known (signing in again).
    pub fn start_at(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.address.update(cx, |s, cx| s.set_value(url.to_owned(), window, cx));
        self.probe(window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.step.clone() {
            Step::Address => self.probe(window, cx),
            Step::Account => self.sign_in(window, cx),
            Step::TwoFactor { ticket } => self.verify(ticket, window, cx),
            Step::Browser => {}
        }
    }

    fn start<T: Send + 'static>(
        &mut self,
        future: impl std::future::Future<Output = T> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.busy = true;
        self.error = None;
        cx.notify();
        let rx = self.core.spawn(future);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(value) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                done(this, value, window, cx);
                cx.notify();
            });
        }));
    }

    fn probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = self.address.read(cx).value().trim().to_owned();
        if input.is_empty() {
            input = "fuwa.chat".into();
        }
        let core = self.core.clone();
        self.start(async move { core.probe(&input).await }, window, cx, |this, result, window, cx| match result {
            Ok((url, node)) => {
                this.url = url;
                let auth = node.auth.clone().unwrap_or_default();
                this.sign_up = !auth.local_sign_in && auth.local_sign_up;
                this.node = Some(node);
                this.step = Step::Account;
                if auth.local_sign_in || auth.local_sign_up {
                    this.username.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            Err(err) => this.error = Some(err.message),
        });
    }

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value().trim().to_owned();
        let password = self.password.read(cx).value().to_string();
        let display = self.display_name.read(cx).value().trim().to_owned();
        if username.is_empty() || password.is_empty() {
            self.error = Some("Fill in your username and password.".into());
            cx.notify();
            return;
        }
        let (core, url) = (self.core.clone(), self.url.clone());
        if self.sign_up {
            self.start(
                async move { core.sign_up(&url, &username, &password, &display).await },
                window,
                cx,
                |this, result, _, cx| match result {
                    Ok(key) => cx.emit(ConnectEvent::Done { key }),
                    Err(err) => this.error = Some(err.message),
                },
            );
        } else {
            self.start(
                async move { core.sign_in(&url, &username, &password).await },
                window,
                cx,
                |this, result, window, cx| match result {
                    Ok(SignIn::Done { key }) => cx.emit(ConnectEvent::Done { key }),
                    Ok(SignIn::TwoFactor { ticket }) => {
                        this.step = Step::TwoFactor { ticket };
                        this.code.update(cx, |s, cx| s.focus(window, cx));
                    }
                    Err(err) => this.error = Some(err.message),
                },
            );
        }
    }

    fn verify(&mut self, ticket: String, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.code.read(cx).value().trim().to_owned();
        let (core, url) = (self.core.clone(), self.url.clone());
        self.start(
            async move { core.verify_two_factor(&url, &ticket, &code).await },
            window,
            cx,
            |this, result, _, cx| match result {
                Ok(key) => cx.emit(ConnectEvent::Done { key }),
                Err(err) => this.error = Some(err.message),
            },
        );
    }

    /// Signs in through the browser: with waifu.dev (`sso` false) or the
    /// instance's identity provider, named `who` while it waits.
    fn in_browser(&mut self, sso: bool, who: String, window: &mut Window, cx: &mut Context<Self>) {
        let (core, url) = (self.core.clone(), self.url.clone());
        self.step = Step::Browser;
        self.browser = who;
        let open_page = crate::ui::open_in_browser;
        self.start(
            async move {
                if sso { core.sso_sign_in(&url, open_page).await } else { core.linked_sign_in(&url, open_page).await }
            },
            window,
            cx,
            |this, result, _, cx| match result {
                Ok((key, _)) => cx.emit(ConnectEvent::Done { key }),
                Err(err) => {
                    this.step = Step::Account;
                    this.error = Some(err.message);
                }
            },
        );
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.busy = false;
        self.error = None;
        self.step = match self.step {
            Step::Address | Step::Account => Step::Address,
            Step::TwoFactor { .. } | Step::Browser => Step::Account,
        };
        cx.notify();
    }
}

impl Render for ConnectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let busy = self.busy;
        let node_name =
            self.node.as_ref().map(|n| n.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.url.clone());
        let auth = self.node.as_ref().and_then(|n| n.auth.clone()).unwrap_or_default();
        let issuer =
            if auth.linked_issuer.is_empty() { "waifu.dev".to_owned() } else { short_host(&auth.linked_issuer) };

        let (step_key, title, sub, body): (&str, String, String, gpui_kit::AnyElement) = match &self.step {
            Step::Address => (
                "address",
                "Say hi to fuwa".into(),
                "Connect to an instance: fuwa.chat, or one a friend runs.".into(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(labeled("Instance address", Input::new(&self.address).large(), &p))
                    .child(
                        primary_button("probe", if busy { "Looking…" } else { "Continue" }, &p)
                            .w_full()
                            .when(busy, |el| el.opacity(0.7))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    )
                    .into_any_element(),
            ),
            Step::Account => {
                let local = auth.local_sign_in || auth.local_sign_up;
                let mut body = div().flex().flex_col().gap(px(14.0));
                if plain_http(&self.url) {
                    body = body.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.0))
                            .p(px(12.0))
                            .rounded(corner(12.0))
                            .bg(alpha(p.destructive, 0.1))
                            .text_color(p.destructive)
                            .text_sm()
                            .child(icon("triangle-alert").size(px(16.0)).mt(px(2.0)))
                            .child(div().flex_1().child(
                                "This instance doesn't use https. Your password, your messages and your address \
                                 travel unprotected: anyone on the network between you can read them.",
                            )),
                    );
                }
                let sso = auth.sso_sign_in || auth.sso_sign_up;
                if sso {
                    let name =
                        if auth.sso_name.is_empty() { "your organization".to_owned() } else { auth.sso_name.clone() };
                    let who = name.clone();
                    body = body.child(
                        primary_button("sso", format!("Continue with {name}"), &p)
                            .w_full()
                            .child(icon("building").size(px(16.0)))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.in_browser(true, who.clone(), window, cx)),
                            ),
                    );
                    // The provider's site sees the address of whoever signs in there, so it's named first.
                    if !auth.sso_host.is_empty() {
                        body = body.child(motion::rise(
                            div().child(crate::ui::overlay::host_notice(&auth.sso_host, &p)),
                            "sso-host",
                            Duration::from_millis(100),
                            4.0,
                        ));
                    }
                }
                if auth.linked_sign_in {
                    let who = issuer.clone();
                    body = body.child(
                        primary_button("linked", format!("Continue with {issuer}"), &p)
                            .w_full()
                            .when(sso, |el| el.bg(p.secondary).text_color(p.foreground).shadow(Vec::new()))
                            .child(icon("external-link").size(px(16.0)))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.in_browser(false, who.clone(), window, cx)),
                            ),
                    );
                }
                if (auth.linked_sign_in || sso) && local {
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .text_xs()
                            .text_color(p.muted_foreground)
                            .child(div().flex_1().h(px(1.0)).bg(p.border))
                            .child("OR A LOCAL ACCOUNT")
                            .child(div().flex_1().h(px(1.0)).bg(p.border)),
                    );
                }
                if local {
                    if self.sign_up {
                        body = body.child(labeled("Display name", Input::new(&self.display_name).large(), &p));
                    }
                    body = body
                        .child(labeled("Username", Input::new(&self.username).large(), &p))
                        .child(labeled("Password", Input::new(&self.password).large().mask_toggle(), &p))
                        .child(
                            primary_button(
                                "local",
                                match (busy, self.sign_up) {
                                    (true, _) => "One moment…",
                                    (false, true) => "Create my account",
                                    (false, false) => "Sign in",
                                },
                                &p,
                            )
                            .w_full()
                            .when(auth.linked_sign_in || sso, |el| {
                                el.bg(p.secondary).text_color(p.foreground).shadow(Vec::new())
                            })
                            .when(busy, |el| el.opacity(0.7))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                        );
                    if auth.local_sign_in && auth.local_sign_up {
                        let sign_up = self.sign_up;
                        body = body.child(
                            div()
                                .id("toggle-mode")
                                .text_sm()
                                .text_color(p.primary)
                                .font_weight(FontWeight::BOLD)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(if sign_up {
                                    "I already have an account"
                                } else {
                                    "New here? Create an account"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.sign_up = !this.sign_up;
                                    this.error = None;
                                    cx.notify();
                                })),
                        );
                    }
                }
                if !local && !auth.linked_sign_in && !sso {
                    body = body.child(
                        div()
                            .text_color(p.muted_foreground)
                            .child("This instance isn't letting anyone sign in right now."),
                    );
                }
                (
                    "account",
                    node_name.clone(),
                    if self.sign_up { "Make an account here.".into() } else { "Sign in to keep chatting.".into() },
                    body.into_any_element(),
                )
            }
            Step::TwoFactor { .. } => (
                "code",
                "Two-step sign-in".into(),
                "Type the code from your authenticator app.".into(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(labeled("Code", Input::new(&self.code).large(), &p))
                    .child(
                        primary_button("verify", if busy { "Checking…" } else { "Verify" }, &p)
                            .w_full()
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    )
                    .into_any_element(),
            ),
            Step::Browser => (
                "browser",
                "Finish in your browser".into(),
                format!("We opened {} in your browser. Come back here when it says you're done.", self.browser),
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(16.0))
                    .py(px(8.0))
                    .child(dots(&p))
                    .child(
                        soft_button("cancel-linked", "Cancel", &p)
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    )
                    .into_any_element(),
            ),
        };

        let panel = card(&p)
            .w(px(420.0))
            .p(px(28.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(self.step != Step::Address, |el| {
                        el.child(
                            icon_button("back", "arrow-left", &p).on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    })
                    .child(div().flex_1())
                    .when(self.can_cancel, |el| {
                        el.child(
                            icon_button("connect-close", "x", &p)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(ConnectEvent::Cancel))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.0))
                    .text_center()
                    .child(motion::ambient(
                        div().child(fuwa_mark(72.0, &p)),
                        "connect-bob",
                        Duration::from_millis(3000),
                        window,
                        |el, t| el.relative().top(px((t * std::f32::consts::TAU).sin() * 4.0)),
                    ))
                    .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(title))
                    .child(div().text_color(p.muted_foreground).child(sub)),
            )
            .child(motion::rise(
                div().child(body),
                SharedString::from(format!("connect-{step_key}")),
                Duration::ZERO,
                14.0,
            ))
            .when_some(error_line(self.error.as_deref(), &p), |el, e| el.child(e));

        // Two soft glows behind the card, drifting.
        let glow = |id: &'static str, color, x: f32, y: f32, period: u64| {
            let el = div()
                .absolute()
                .left(gpui_kit::relative(x))
                .top(gpui_kit::relative(y))
                .size(px(360.0))
                .rounded_full()
                .bg(color);
            motion::ambient(el, id, Duration::from_millis(period), window, move |el, t| {
                let a = t * std::f32::consts::TAU;
                el.ml(px(a.cos() * 30.0 - 180.0)).mt(px(a.sin() * 24.0 - 180.0))
            })
        };
        div()
            .id("connect")
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(alpha(p.background, if self.can_cancel { 0.0 } else { 1.0 }))
            .child(glow("glow-a", alpha(p.primary, 0.11), 0.25, 0.3, 9000))
            .child(glow("glow-b", alpha(p.glow, 0.11), 0.75, 0.7, 11000))
            .child(div().absolute().inset_0().flex().items_center().justify_center().child(motion::rise(
                panel,
                "connect-panel",
                Duration::ZERO,
                24.0,
            )))
            .child(
                div()
                    .absolute()
                    .bottom(px(16.0))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .text_xs()
                    .text_color(mix(p.muted_foreground, p.background, 0.3))
                    .child(format!("fuwa desktop {}", env!("CARGO_PKG_VERSION"))),
            )
    }
}

/// Three dots taking turns, while the browser does its part.
fn dots(p: &crate::ui::theme::Palette) -> impl IntoElement {
    div().flex().gap(px(8.0)).children((0..3).map(|n| {
        div().size(px(10.0)).rounded_full().bg(p.primary).with_animation(
            SharedString::from(format!("dot-{n}")),
            Animation::new(Duration::from_millis(1200)).repeat(),
            move |el, t| {
                let phase = ((t - n as f32 * 0.18).rem_euclid(1.0) * std::f32::consts::TAU).sin().max(0.0);
                el.opacity(0.35 + 0.65 * phase).relative().top(px(-6.0 * phase))
            },
        )
    }))
}

/// Whether an address is plain http somewhere other than this computer.
fn plain_http(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    parsed.scheme() == "http"
        && !matches!(parsed.host(), Some(url::Host::Domain("localhost")))
        && !matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        && !matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
}

fn short_host(url: &str) -> String {
    let host =
        url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| url.to_owned());
    // People know waifu.dev, not its API's address.
    host.strip_prefix("api.").map(str::to_owned).unwrap_or(host)
}
