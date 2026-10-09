//! Connecting to an instance: its address, then signing in (with waifu.dev
//! or the instance's identity provider in the browser, or a username and
//! password kept on that instance), then the two-step code if it's on. Like
//! the web app's `components/Connect.tsx`: on its own it's the first-run
//! welcome (`pages/Welcome.tsx`, the pitch beside the form), and over the app
//! it's the "Connect to a fuwa server" dialog (`AddInstanceDialog.tsx`) or
//! "Add an account" (`AccountSwitcher.tsx`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Div, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render, Rgba, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};

use crate::core::api::instance_key;
use crate::core::config::SavedAccount;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::providers::{PendingProvider, ProviderAnswer};
use crate::core::{Core, SignIn};
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_3xl, radius_lg, radius_md, radius_xl};
use crate::ui::widgets::{avatar, fuwa_mark, icon, pal};

/// Where "one you run" goes.
const SELF_HOSTING: &str = "https://github.com/waifu-devs/fuwa/blob/master/docs/self-hosting.md";
/// Addresses Waifu Devs runs fuwa on (the web's `lib/hosted.ts`).
const HOSTED_DOMAINS: [&str; 1] = ["fuwa.chat"];

/// The welcome's rotating line, one after another.
const PHRASES: [&str; 4] = [
    "connect.welcome.phrase.ourServers",
    "connect.welcome.phrase.ownBox",
    "connect.welcome.phrase.friends",
    "connect.welcome.phrase.instances",
];

const FEATURES: [(&str, &str); 3] = [
    ("layers", "connect.welcome.feature.oneApp"),
    ("server-cog", "connect.welcome.feature.selfHost"),
    ("database", "connect.welcome.feature.database"),
];

pub enum ConnectEvent {
    Done { key: String },
    Cancel,
}

#[derive(Clone, PartialEq)]
enum Step {
    Address,
    Account,
    TwoFactor {
        ticket: String,
        backup: bool,
    },
    Browser,
    /// Someone new signing in with Google, X or Twitch: their username first.
    NewAccount {
        pending: PendingProvider,
        account: pb::NewProviderAccount,
    },
}

/// What the form is for, which decides its frame and title.
#[derive(Clone, PartialEq)]
enum Mode {
    /// The first run: the welcome page around it.
    Welcome,
    /// "Connect to a fuwa server", over the app.
    Add,
    /// One more account on an instance already here, by its name.
    Account(String),
}

pub struct ConnectView {
    core: Arc<Core>,
    pub can_cancel: bool,
    mode: Mode,
    step: Step,
    address: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    display_name: Entity<InputState>,
    code: Entity<InputState>,
    backup: Entity<InputState>,
    /// A new account's username, after a provider sign-in.
    new_username: Entity<InputState>,
    /// Bring the provider's name and picture into the new account.
    use_profile: bool,
    url: String,
    node: Option<pb::Node>,
    sign_up: bool,
    busy: bool,
    error: Option<String>,
    /// When the last try failed, for the fields' shake.
    shook: Option<Instant>,
    /// Who the browser is signing you in with, while it is.
    browser: String,
    /// When the view opened, for the welcome's rotating line.
    opened: Instant,
    task: Option<Task<()>>,
    _ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConnectEvent> for ConnectView {}

impl ConnectView {
    pub fn new(core: Arc<Core>, can_cancel: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("chat.example.com"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder(t("connect.account.usernameHint")));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let display_name = cx.new(|cx| InputState::new(window, cx).placeholder(t("connect.account.displayNameHint")));
        let code = cx.new(|cx| InputState::new(window, cx));
        let backup = cx.new(|cx| InputState::new(window, cx).placeholder("abcd-efgh"));
        let new_username = cx.new(|cx| InputState::new(window, cx).placeholder(t("connect.account.usernameHint")));
        let mut subs = Vec::new();
        for input in [&address, &username, &password, &display_name, &backup, &new_username] {
            subs.push(cx.subscribe_in(input, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
                cx.notify();
            }));
        }
        // The six digits send themselves once they're all there.
        subs.push(cx.subscribe_in(&code, window, |this: &mut Self, state, event: &InputEvent, window, cx| {
            if let InputEvent::Change = event {
                let typed = state.read(cx).value().to_string();
                let digits: String = typed.chars().filter(char::is_ascii_digit).take(6).collect();
                if digits != typed {
                    state.update(cx, |s, cx| s.set_value(digits.clone(), window, cx));
                }
                if digits.len() == 6 && !this.busy {
                    this.submit(window, cx);
                }
                cx.notify();
            }
        }));
        address.update(cx, |s, cx| s.focus(window, cx));
        // The welcome's line turns every 2.4 seconds.
        let ticker = (!can_cancel).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            })
        });
        Self {
            core,
            can_cancel,
            mode: if can_cancel { Mode::Add } else { Mode::Welcome },
            step: Step::Address,
            address,
            username,
            password,
            display_name,
            code,
            backup,
            new_username,
            use_profile: true,
            url: String::new(),
            node: None,
            sign_up: false,
            busy: false,
            error: None,
            shook: None,
            browser: String::new(),
            opened: Instant::now(),
            task: None,
            _ticker: ticker,
            _subscriptions: subs,
        }
    }

    /// Starts at an address already known (signing in again).
    pub fn start_at(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.address.update(cx, |s, cx| s.set_value(url.to_owned(), window, cx));
        self.probe(window, cx);
    }

    /// Signing in to one more account on an instance already here.
    pub fn add_account(&mut self, url: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Account(name.to_owned());
        self.start_at(url, window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.step.clone() {
            Step::Address => self.probe(window, cx),
            Step::Account => self.sign_in(window, cx),
            Step::TwoFactor { ticket, backup } => self.verify(ticket, backup, window, cx),
            Step::Browser => {}
            Step::NewAccount { pending, .. } => self.create_account(pending, window, cx),
        }
    }

    /// Signs in with Google, X or Twitch in the browser (the web's `ProviderSignIn`).
    fn in_provider(&mut self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let (core, url) = (self.core.clone(), self.url.clone());
        self.step = Step::Browser;
        self.browser = name;
        let open_page = crate::ui::open_in_browser;
        self.start(
            async move { core.provider_sign_in(&url, &id, open_page).await },
            window,
            cx,
            |this, result, window, cx| this.provider_answer(result, window, cx),
        );
    }

    /// Someone new picked their username: make the account and sign in.
    fn create_account(&mut self, pending: PendingProvider, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.new_username.read(cx).value().trim().to_lowercase();
        if username.is_empty() {
            return;
        }
        let (core, use_profile) = (self.core.clone(), self.use_profile);
        self.start(
            async move { core.finish_provider(pending, Some((username, use_profile))).await },
            window,
            cx,
            |this, result, window, cx| this.provider_answer(result, window, cx),
        );
    }

    fn provider_answer(
        &mut self,
        result: Result<ProviderAnswer, crate::core::api::Problem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(ProviderAnswer::Done { key }) => cx.emit(ConnectEvent::Done { key }),
            Ok(ProviderAnswer::TwoFactor { ticket }) => {
                self.step = Step::TwoFactor { ticket, backup: false };
                self.code.update(cx, |s, cx| s.focus(window, cx));
            }
            Ok(ProviderAnswer::NewAccount { pending, account }) => {
                let suggested = account.suggested_username.clone();
                self.new_username.update(cx, |s, cx| {
                    if s.value().is_empty() {
                        s.set_value(suggested, window, cx);
                    }
                    s.focus(window, cx);
                });
                self.step = Step::NewAccount { pending, account };
            }
            Err(err) => {
                if matches!(self.step, Step::Browser) {
                    self.step = Step::Account;
                }
                self.fail(err.message);
            }
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

    fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.shook = Some(Instant::now());
    }

    fn probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.address.read(cx).value().trim().to_owned();
        if input.is_empty() {
            return;
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
                    let field = if this.sign_up { &this.display_name } else { &this.username };
                    field.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            Err(err) => {
                // Nothing answered there: the web's words for it.
                let problem = if matches!(err.code, tonic::Code::Unavailable | tonic::Code::Unknown) {
                    t("system.connection.unreachable")
                } else {
                    err.message
                };
                this.fail(t_with("connect.where.notFound", &[("problem", Arg::Str(&problem))]))
            }
        });
    }

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.username.read(cx).value().trim().to_lowercase();
        let password = self.password.read(cx).value().to_string();
        let display = self.display_name.read(cx).value().trim().to_owned();
        if username.is_empty() || password.is_empty() {
            self.fail("Fill in your username and password.".into());
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
                    Err(err) => this.fail(err.message),
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
                        this.password.update(cx, |s, cx| s.set_value(String::new(), window, cx));
                        this.step = Step::TwoFactor { ticket, backup: false };
                        this.code.update(cx, |s, cx| s.focus(window, cx));
                    }
                    Err(err) => this.fail(err.message),
                },
            );
        }
    }

    fn verify(&mut self, ticket: String, backup: bool, window: &mut Window, cx: &mut Context<Self>) {
        let field = if backup { &self.backup } else { &self.code };
        let code = field.read(cx).value().trim().to_owned();
        if code.is_empty() {
            return;
        }
        let (core, url) = (self.core.clone(), self.url.clone());
        self.start(
            async move { core.verify_two_factor(&url, &ticket, &code).await },
            window,
            cx,
            |this, result, window, cx| match result {
                Ok(key) => cx.emit(ConnectEvent::Done { key }),
                Err(err) => {
                    // A sign-in that ran out goes back to the password.
                    if err.message.contains("ran out") {
                        this.step = Step::Account;
                    } else {
                        this.code.update(cx, |s, cx| s.set_value(String::new(), window, cx));
                    }
                    this.fail(err.message);
                }
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
            Step::TwoFactor { .. } | Step::Browser | Step::NewAccount { .. } => Step::Account,
        };
        cx.notify();
    }

    /// The other accounts kept on the instance at hand, to carry on as without signing in again.
    fn others(&self) -> Vec<SavedAccount> {
        if self.url.is_empty() {
            return Vec::new();
        }
        let key = instance_key(&self.url);
        let active = self.core.shared.read(|s| s.instance(&key).and_then(|i| i.me.as_ref().map(|m| m.id.clone())));
        self.core
            .kept_accounts(&key)
            .into_iter()
            .filter(|a| !a.user_id.is_empty() && Some(&a.user_id) != active.as_ref())
            .collect()
    }

    // ───────────────────────── Pieces ─────────────────────────

    /// How far a field shakes now, after a failed try (the web's `.shake`).
    fn shake(&self) -> f32 {
        let Some(at) = self.shook else { return 0.0 };
        let t = at.elapsed().as_secs_f32() / 0.4;
        if t >= 1.0 {
            return 0.0;
        }
        // 20%, 60%: -4px; 40%, 80%: 4px.
        let key = [0.0, -4.0, 4.0, -4.0, 4.0, 0.0];
        let at = t * 5.0;
        let n = at.floor() as usize;
        key[n] + (key[(n + 1).min(5)] - key[n]) * (at - n as f32)
    }

    fn where_body(&mut self, window: &mut Window, cx: &mut Context<Self>, p: &Palette) -> AnyElement {
        let empty = self.address.read(cx).value().trim().is_empty();
        let shake = self.shake();
        if shake != 0.0 {
            window.request_animation_frame();
        }
        let (before, after) = split_link(&t_with("connect.where.anyServer", &[("link", Arg::Str("{link}"))]));
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(label(t("connect.where.address")))
                    .child(div().relative().left(px(shake)).child(field(
                        &self.address,
                        Some("server"),
                        self.error.is_some(),
                        window,
                        cx,
                        p,
                    )))
                    .when_some(self.error.clone(), |el, e| el.child(error_text(e, false, p))),
            )
            .child(
                big_button("probe", if self.busy { t("connect.where.looking") } else { t("common.continue") }, p)
                    .when(self.busy, |el| el.child(spinner(p.primary_foreground)))
                    .when(empty || self.busy, |el| el.opacity(0.5))
                    .when(!empty && !self.busy, |el| {
                        el.on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(before)
                    .child(
                        div()
                            .id("self-hosting")
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.primary)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(|_, _, cx| crate::ui::text::open_link(SELF_HOSTING, cx))
                            .child(t("connect.where.anyServerLink")),
                    )
                    .child(after),
            )
            .into_any_element()
    }

    fn account_body(&mut self, window: &mut Window, cx: &mut Context<Self>, p: &Palette) -> AnyElement {
        let auth = self.node.as_ref().and_then(|n| n.auth.clone()).unwrap_or_default();
        let (can_in, can_up) = (auth.local_sign_in, auth.local_sign_up);
        let sso = auth.sso_sign_in;
        let others = sso || auth.linked_sign_in || !auth.providers.is_empty();
        let mut body = div().flex().flex_col().gap(px(16.0)).child(self.header(cx, p));
        if plain_http(&self.url) {
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .p(px(12.0))
                    .rounded(radius_xl())
                    .bg(alpha(p.destructive, 0.1))
                    .text_color(p.destructive)
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .child(icon("triangle-alert").size(px(16.0)).mt(px(2.0)))
                    .child(div().flex_1().child(
                        "This instance doesn't use https. Your password, your messages and your address \
                         travel unprotected: anyone on the network between you can read them.",
                    )),
            );
        }
        let kept = self.others();
        if !kept.is_empty() {
            body = body.child(self.continue_as(&kept, cx, p));
        }
        if others {
            body = body.child(self.provider_buttons(&auth, cx, p));
        }
        if !can_in && !can_up {
            if !others {
                body = body.child(
                    div()
                        .p(px(16.0))
                        .rounded(radius_2xl())
                        .bg(p.muted)
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .child(t("connect.account.noSignIns")),
                );
            }
            return body
                .when_some(self.error.clone(), |el, e| el.child(error_text(e, true, p)))
                .when(others, |el| el.children(self.agreement(p)))
                .into_any_element();
        }
        if others {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(div().flex_1().h(px(1.0)).bg(p.border))
                    .child(t("connect.account.orPassword").to_uppercase())
                    .child(div().flex_1().h(px(1.0)).bg(p.border)),
            );
        }
        // Sign in or create an account, as tabs.
        let tabs = {
            let x = motion::follow("connect-tab", if self.sign_up { 1.0 } else { 0.0 }, window, cx);
            let tab = |id: &'static str, label: String, on: bool, enabled: bool| {
                div()
                    .id(id)
                    .relative()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if on { p.foreground } else { p.muted_foreground })
                    .when(!enabled, |el| el.opacity(0.5))
                    .when(enabled, |el| el.cursor_pointer())
                    .child(label)
            };
            div()
                .relative()
                .w_full()
                .h(px(36.0))
                .p(px(3.0))
                .rounded(radius_lg())
                .bg(p.muted)
                .flex()
                .child(
                    // The highlight glides to the tab that's on.
                    div().absolute().top(px(3.0)).bottom(px(3.0)).left(px(3.0)).right(px(3.0)).child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom(px(1.0))
                            .w(gpui_kit::relative(0.5))
                            .left(gpui_kit::relative(0.5 * x))
                            .rounded(radius_md())
                            .bg(p.background)
                            .shadow_sm(),
                    ),
                )
                .child(tab("tab-in", t("connect.account.signIn"), !self.sign_up, can_in).when(can_in, |el| {
                    el.on_click(cx.listener(|this, _, window, cx| {
                        this.sign_up = false;
                        this.error = None;
                        this.username.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    }))
                }))
                .child(tab("tab-up", t("connect.account.signUp"), self.sign_up, can_up).when(can_up, |el| {
                    el.on_click(cx.listener(|this, _, window, cx| {
                        this.sign_up = true;
                        this.error = None;
                        this.display_name.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    }))
                }))
        };
        let mut tabbed = div().flex().flex_col().gap(px(8.0)).child(tabs);
        if self.sign_up {
            tabbed = tabbed.child(motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pt(px(8.0))
                    .child(label(t("connect.account.displayName")))
                    .child(field(&self.display_name, None, false, window, cx, p)),
                "connect-up",
                Duration::ZERO,
                -4.0,
            ));
        } else if !can_up {
            tabbed = tabbed.child(motion::rise(
                div()
                    .pt(px(8.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("connect.account.noSignUps")),
                "connect-noup",
                Duration::ZERO,
                -4.0,
            ));
        }
        let shake = self.shake();
        if shake != 0.0 {
            window.request_animation_frame();
        }
        body.child(tabbed)
            .child(
                div()
                    .relative()
                    .left(px(shake))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(label(t("connect.account.username")))
                    .child(field(&self.username, None, false, window, cx, p))
                    .child(label(t("connect.account.password")).mt(px(8.0)))
                    .child(field(&self.password, None, false, window, cx, p)),
            )
            .when_some(self.error.clone(), |el, e| el.child(error_text(e, false, p)))
            .child(
                big_button(
                    "local",
                    if self.sign_up { t("connect.account.signUp") } else { t("connect.account.signIn") },
                    p,
                )
                .when(self.busy, |el| el.opacity(0.5).child(spinner(p.primary_foreground)))
                .when(!self.busy, |el| el.on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))),
            )
            .children(self.agreement(p))
            .into_any_element()
    }

    /// "By continuing, you agree to the Terms of Service and the Privacy
    /// Policy.", under the ways in, on Waifu Devs' own instances alone. The
    /// pages open in the browser.
    fn agreement(&self, p: &Palette) -> Option<AnyElement> {
        if !hosted_by_us(&self.url) {
            return None;
        }
        let base = self.url.trim_end_matches('/').to_owned();
        let text = t_with("connect.legal.agree", &[("terms", Arg::Str("{terms}")), ("privacy", Arg::Str("{privacy}"))]);
        let mut line = div()
            .flex()
            .flex_wrap()
            .justify_center()
            .text_size(px(12.0))
            .line_height(px(16.0))
            .text_color(p.muted_foreground);
        for piece in agreement_pieces(&text) {
            line = match piece {
                Piece::Word(word) => line.child(word),
                Piece::Link(page) => {
                    let url = format!("{base}/{page}");
                    let label =
                        if page == "terms" { t("connect.legal.termsTitle") } else { t("connect.legal.privacyTitle") };
                    line.child(
                        div()
                            .id(page)
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.primary)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(move |_, _, cx| crate::ui::text::open_link(&url, cx))
                            .child(label),
                    )
                }
            };
        }
        Some(line.into_any_element())
    }

    /// The instance's name and address over the sign-in, with the way back.
    fn header(&self, cx: &mut Context<Self>, p: &Palette) -> Div {
        let node = self.node.clone().unwrap_or_default();
        let streamer = self.core.prefs().streamer_mode;
        let address = if streamer { "address hidden".to_owned() } else { instance_key(&self.url) };
        let commit: String = node.build.as_ref().map(|b| b.commit.chars().take(7).collect()).unwrap_or_default();
        let build = if commit.is_empty() {
            format!("fuwa {}", node.version)
        } else {
            format!("fuwa {} · {commit}", node.version)
        };
        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .when(!matches!(self.mode, Mode::Account(_)), |el| {
                el.child(round_button("back", "arrow-left", p).on_click(cx.listener(|this, _, _, cx| this.back(cx))))
            })
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .text_size(px(16.0))
                            .line_height(px(24.0))
                            .child(if node.name.is_empty() { address.clone() } else { node.name.clone() }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(format!("{address} · {build}")),
                    ),
            )
    }

    /// The web's `ContinueAs`: the other accounts kept here.
    fn continue_as(&self, kept: &[SavedAccount], cx: &mut Context<Self>, p: &Palette) -> Div {
        let streamer = self.core.prefs().streamer_mode;
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, a) in kept.iter().enumerate() {
            let user = crate::core::accounts::user_of(a);
            let id = format!("as-{}", a.user_id);
            let (key, uid) = (instance_key(&self.url), a.user_id.clone());
            let border = p.border;
            let hover_border = p.primary;
            let (glow, lit) = (alpha(p.primary, 1.0), p.primary);
            list = list.child(motion::rise(
                div()
                    .id(SharedString::from(id.clone()))
                    .group(SharedString::from(id.clone()))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(border)
                    .bg(alpha(p.background, 0.6))
                    .cursor_pointer()
                    // The web's `.card-pop`: the border lights and a glow drops under it.
                    .hover(move |s| {
                        s.border_color(hover_border).shadow(vec![gpui_kit::BoxShadow {
                            color: glow,
                            offset: gpui_kit::point(px(0.0), px(18.0)),
                            blur_radius: px(20.0),
                            spread_radius: px(-22.0),
                            inset: false,
                        }])
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.core.switch_account(&key, &uid) {
                            cx.emit(ConnectEvent::Done { key: key.clone() });
                        }
                    }))
                    // `group-hover:-rotate-6 group-hover:scale-110`.
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}|avatar")))
                            .group_hover(SharedString::from(id.clone()), |s| {
                                s.rotate(gpui_kit::radians(-6f32.to_radians())).scale(1.1)
                            })
                            .child(avatar(Some(&user), 40.0, p)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .child(crate::core::store::user_name(&user)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(if streamer {
                                        format!("@{}", crate::core::accounts::mask_name(&a.username))
                                    } else {
                                        format!("@{}", a.username)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}|arrow")))
                            .text_color(p.muted_foreground)
                            .group_hover(SharedString::from(id.clone()), move |s| {
                                s.translate_x(px(4.0)).text_color(lit)
                            })
                            .child(icon("arrow-right").size(px(16.0))),
                    ),
                SharedString::from(format!("as-in-{n}")),
                Duration::from_millis(40 * n as u64),
                8.0,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().text_size(px(14.0)).font_weight(FontWeight::BOLD).child(t("connect.accounts.continueAs")))
            .child(list)
            .child(
                div()
                    .mt(px(8.0))
                    .text_center()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t("connect.accounts.orAnother")),
            )
    }

    /// Single sign-on, then waifu.dev: one dark "Continue with …" button each.
    fn provider_buttons(&self, auth: &pb::AuthMethods, cx: &mut Context<Self>, p: &Palette) -> Div {
        let mut col = div().flex().flex_col().gap(px(16.0));
        if auth.sso_sign_in {
            let name =
                if auth.sso_name.is_empty() { t("connect.provider.yourOrganization") } else { auth.sso_name.clone() };
            let who = name.clone();
            let mut block = div().flex().flex_col().gap(px(6.0)).child(
                provider_button("sso", "building", &name, p)
                    .on_click(cx.listener(move |this, _, window, cx| this.in_browser(true, who.clone(), window, cx))),
            );
            if !auth.sso_sign_up {
                block = block.child(closed_note(&name, p));
            }
            // The provider's site sees the address of whoever signs in there, so it's named first.
            if !auth.sso_host.is_empty() {
                block = block.child(motion::rise(
                    div()
                        .flex()
                        .justify_center()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(icon("globe").size(px(14.0)).mr(px(4.0)))
                        .child(crate::ui::text::hint_line(
                            &t_with("connect.provider.signsYouInAt", &[("host", Arg::Str("{host}"))]),
                            &[("host", &auth.sso_host)],
                            p,
                        )),
                    "sso-host",
                    Duration::from_millis(100),
                    4.0,
                ));
            }
            col = col.child(block);
        }
        if auth.linked_sign_in {
            let issuer = issuer_name(&auth.linked_issuer);
            let who = issuer.clone();
            let mut block = div().flex().flex_col().gap(px(8.0)).child(
                provider_button("linked", "flower-2", &issuer, p)
                    .on_click(cx.listener(move |this, _, window, cx| this.in_browser(false, who.clone(), window, cx))),
            );
            if !auth.linked_sign_up {
                block = block.child(closed_note(&issuer, p));
            }
            col = col.child(block);
        }
        // Google, X, Twitch: whichever the instance's admins turned on.
        if !auth.providers.is_empty() {
            let mut social = div().flex().flex_col().gap(px(10.0));
            for (n, provider) in auth.providers.iter().enumerate() {
                let (id, name) = (provider.id.clone(), provider.name.clone());
                let mark = provider_mark(&provider.id, 18.0, p.primary);
                social = social.child(motion::rise(
                    provider_button_with(
                        SharedString::from(format!("provider-{}", provider.id)),
                        mark,
                        &provider.name,
                        p,
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.in_provider(id.clone(), name.clone(), window, cx)),
                    ),
                    SharedString::from(format!("provider-in-{n}")),
                    Duration::from_millis(50 * n as u64),
                    8.0,
                ));
            }
            col = col.child(social);
        }
        col
    }

    fn two_factor_body(
        &mut self,
        backup: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        p: &Palette,
    ) -> AnyElement {
        let shake = self.shake();
        if shake != 0.0 {
            window.request_animation_frame();
        }
        let hint = if backup { t("connect.twoStep.backupHint") } else { t("connect.twoStep.appHint") };
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(round_button("back-2fa", "arrow-left", p).on_click(cx.listener(|this, _, _, cx| this.back(cx))))
            // It pops up from nothing, turning upright, a moment after the page.
            .child(motion::pop(
                div()
                    .size(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius_2xl())
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .child(icon("shield-check").size(px(20.0))),
                "shield-pop",
                0.0,
                -30.0,
                Duration::from_millis(100),
            ))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("connect.twoStep.title")))
                    .child(div().text_size(px(12.0)).line_height(px(16.0)).text_color(p.muted_foreground).child(hint)),
            );
        let entry: AnyElement = if backup {
            motion::rise(
                div()
                    .relative()
                    .left(px(shake))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(label(t("connect.twoStep.backupCode")))
                    .child(field(&self.backup, None, false, window, cx, p)),
                "backup-in",
                Duration::ZERO,
                8.0,
            )
            .into_any_element()
        } else {
            let typed: Vec<char> = self.code.read(cx).value().chars().collect();
            let focused = self.code.read(cx).focus_handle(cx).is_focused(window);
            let caret = typed.len().min(5);
            let mut boxes = div().flex().gap(px(8.0));
            for n in 0..6 {
                let digit = typed.get(n).copied();
                let here = focused && n == caret && !self.busy;
                boxes = boxes.child(
                    div()
                        .relative()
                        .when(n == 3, |el| el.ml(px(8.0)))
                        .w(px(48.0))
                        .h(px(56.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius_xl())
                        .border_2()
                        .border_color(if digit.is_some() { alpha(p.primary, 0.5) } else { p.border.into() })
                        .bg(p.background)
                        .text_size(px(24.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .when(self.busy, |el| el.opacity(0.6))
                        .when(here, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .inset(px(-2.0))
                                    .rounded(radius_xl())
                                    .border_2()
                                    .border_color(p.primary)
                                    .shadow(vec![gpui_kit::BoxShadow {
                                        color: alpha(p.primary, 0.2),
                                        offset: gpui_kit::point(px(0.0), px(0.0)),
                                        blur_radius: px(0.0),
                                        spread_radius: px(4.0),
                                        inset: false,
                                    }]),
                            )
                        })
                        .when_some(digit, |el, d| {
                            el.child(motion::rise(
                                div().child(d.to_string()),
                                SharedString::from(format!("digit-{n}-{d}")),
                                Duration::ZERO,
                                8.0,
                            ))
                        })
                        .when(digit.is_none() && here, |el| {
                            el.child(div().w(px(2.0)).h(px(24.0)).rounded_full().bg(p.primary).with_animation(
                                "caret",
                                Animation::new(Duration::from_millis(1000)).repeat(),
                                |el, t| el.opacity(if t < 0.5 { 1.0 } else { 0.0 }),
                            ))
                        }),
                );
            }
            // The typing goes to a field nobody sees, under the boxes.
            div()
                .flex()
                .justify_center()
                .child(
                    div()
                        .relative()
                        .left(px(shake))
                        .child(boxes)
                        .child(div().absolute().inset_0().opacity(0.0).child(Input::new(&self.code).h_full())),
                )
                .into_any_element()
        };
        let toggle = div()
            .id("toggle-backup")
            .self_center()
            .text_size(px(14.0))
            .font_weight(FontWeight::BOLD)
            .text_color(p.primary)
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Step::TwoFactor { ticket, .. } = this.step.clone() {
                    this.step = Step::TwoFactor { ticket, backup: !backup };
                    this.error = None;
                    let field = if backup { &this.code } else { &this.backup };
                    field.update(cx, |s, cx| {
                        s.set_value(String::new(), window, cx);
                        s.focus(window, cx);
                    });
                }
                cx.notify();
            }))
            .child(if backup { t("connect.twoStep.useApp") } else { t("connect.twoStep.useBackup") });
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(head)
            .child(entry)
            .when_some(self.error.clone(), |el, e| el.child(error_text(e, true, p)))
            .when(backup, |el| {
                el.child(
                    big_button("verify", t("connect.account.signIn"), p)
                        .child(if self.busy {
                            spinner(p.primary_foreground).into_any_element()
                        } else {
                            icon("key-round").size(px(16.0)).into_any_element()
                        })
                        .when(self.busy, |el| el.opacity(0.5))
                        .when(!self.busy, |el| el.on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))),
                )
            })
            .child(toggle)
            .into_any_element()
    }

    /// Someone new after a provider sign-in: the web's `ProviderDone` new-account form.
    fn new_account_body(
        &mut self,
        pending: &PendingProvider,
        account: &pb::NewProviderAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
        p: &Palette,
    ) -> AnyElement {
        let empty = self.new_username.read(cx).value().trim().is_empty();
        let on = self.use_profile;
        let streamer = self.core.prefs().streamer_mode;
        let what = if account.display_name.is_empty() {
            t("connect.providerDone.pictureOnly")
        } else if streamer {
            "address hidden".to_owned()
        } else {
            account.display_name.clone()
        };
        let hover = alpha(p.muted, 0.7);
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .text_center()
                    .child(
                        div()
                            .size(px(64.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius_3xl())
                            .bg(alpha(p.primary, 0.15))
                            .text_color(p.primary)
                            .child(icon("user-plus").size(px(28.0))),
                    )
                    .child(
                        div()
                            .text_size(px(20.0))
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("connect.providerDone.newTitle")),
                    )
                    .child(div().text_size(px(14.0)).line_height(px(20.0)).text_color(p.muted_foreground).child(
                        t_with("connect.providerDone.newNote", &[("provider", Arg::Str(&account.provider_name))]),
                    )),
            )
            .child(div().flex().flex_col().gap(px(8.0)).child(label(t("connect.account.username"))).child(field(
                &self.new_username,
                None,
                false,
                window,
                cx,
                p,
            )))
            .child(
                div()
                    .id("use-profile")
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(alpha(p.muted, 0.4))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.use_profile = !this.use_profile;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(36.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius_xl())
                            .bg(p.foreground)
                            .child(provider_mark(&pending.provider, 16.0, p.primary)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(14.0)).font_weight(FontWeight::BOLD).child(t_with(
                                "connect.providerDone.useProfile",
                                &[("provider", Arg::Str(&account.provider_name))],
                            )))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(what),
                            ),
                    )
                    .child(
                        div()
                            .size(px(16.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(if on { p.primary } else { p.border })
                            .when(on, |el| {
                                el.bg(p.primary).child(icon("check").size(px(12.0)).text_color(p.primary_foreground))
                            }),
                    ),
            )
            .when_some(self.error.clone(), |el, e| el.child(error_text(e, true, p)))
            .child(
                big_button("create-account", t("connect.providerDone.create"), p)
                    .child(if self.busy {
                        spinner(p.primary_foreground).into_any_element()
                    } else {
                        icon("arrow-right").size(px(16.0)).into_any_element()
                    })
                    .when(empty || self.busy, |el| el.opacity(0.5))
                    .when(!empty && !self.busy, |el| {
                        el.on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                    }),
            )
            .children(self.agreement(p))
            .into_any_element()
    }

    fn browser_body(&mut self, cx: &mut Context<Self>, p: &Palette) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.header(cx, p))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(16.0))
                    .py(px(12.0))
                    .child(dots(p))
                    .child(
                        div()
                            .text_center()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t_with("connect.provider.leaving", &[("name", Arg::Str(&self.browser))])),
                    )
                    .child(
                        round_button("cancel-browser", "x", p).on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    ),
            )
            .into_any_element()
    }

    /// The form for the step at hand.
    fn form(&mut self, window: &mut Window, cx: &mut Context<Self>, p: &Palette) -> AnyElement {
        let (key, body) = match self.step.clone() {
            Step::Address => ("where", self.where_body(window, cx, p)),
            Step::Account => ("account", self.account_body(window, cx, p)),
            Step::TwoFactor { backup, .. } => ("two-step", self.two_factor_body(backup, window, cx, p)),
            Step::Browser => ("browser", self.browser_body(cx, p)),
            Step::NewAccount { pending, account } => ("new", self.new_account_body(&pending, &account, window, cx, p)),
        };
        // Steps slide in from the side they come from.
        let from = if key == "where" { -24.0 } else { 24.0 };
        motion::slide_in(div().child(body), SharedString::from(format!("connect-step-{key}")), from).into_any_element()
    }
}

impl Render for ConnectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let form = self.form(window, cx, &p);
        match self.mode.clone() {
            Mode::Welcome => self.welcome(form, window, cx, &p).into_any_element(),
            mode => {
                let (title, about) = match mode {
                    Mode::Account(name) => (
                        t("connect.accounts.addTitle"),
                        t_with("connect.accounts.addAbout", &[("instance", Arg::Str(&name))]),
                    ),
                    _ => (t("workspace.addInstance.title"), t("workspace.addInstance.about")),
                };
                let card = div()
                    .id("connect-dialog")
                    .relative()
                    .w(px(448.0))
                    .max_h(gpui_kit::relative(0.92))
                    .overflow_y_scroll()
                    .p(px(24.0))
                    .rounded(radius_3xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .shadow_2xl()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .mb(px(20.0))
                            .pr(px(32.0))
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .line_height(px(28.0))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .text_size(px(14.0))
                                    .line_height(px(20.0))
                                    .text_color(p.muted_foreground)
                                    .child(about),
                            ),
                    )
                    .child(form)
                    .when(self.can_cancel, |el| {
                        el.child(
                            crate::ui::overlay::dialog_close("connect-close", &p)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(ConnectEvent::Cancel))),
                        )
                    });
                // The web's dialog overlay: the app behind darkened by half, fading in.
                let scrim = div()
                    .id("connect")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.5))
                    .backdrop_blur(px(crate::ui::overlay::SCRIM_BLUR))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.can_cancel {
                            cx.emit(ConnectEvent::Cancel);
                        }
                    }))
                    .child(motion::dialog_in(card, "connect-dialog-in"));
                motion::fade_in(scrim, "connect-fade", Duration::from_millis(200)).into_any_element()
            }
        }
    }
}

impl ConnectView {
    /// The first run (the web's `Welcome.tsx`): what fuwa is, beside where to connect.
    fn welcome(&mut self, form: AnyElement, window: &mut Window, cx: &mut Context<Self>, p: &Palette) -> AnyElement {
        let reduce = cx.reduce_motion();
        // The rotating line: the first phrase after 1.2s, then the next every 2.4s.
        let elapsed = self.opened.elapsed().as_secs_f32();
        let idx = if reduce || elapsed < 1.2 { 0 } else { ((elapsed - 1.2) / 2.4) as usize % PHRASES.len() };
        let phrase = t(PHRASES[idx]);
        let headline = 70.4;
        let hero = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(24.0))
            // The mark springs in from small and tilted as it fades in.
            .child(motion::fade_in(
                div().child(motion::pop(
                    div().relative().size(px(80.0)).child(fuwa_mark(80.0, p)),
                    "welcome-mark",
                    0.6,
                    -12.0,
                    Duration::ZERO,
                )),
                "welcome-mark-in",
                Duration::from_millis(300),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(motion::rise(
                        tracked(&t("connect.welcome.headline"), headline, None)
                            .line_height(px(headline * 0.95))
                            .text_color(p.foreground),
                        "welcome-h1",
                        Duration::from_millis(100),
                        24.0,
                    ))
                    .child(motion::rise(
                        div()
                            .mt(px(4.0))
                            .h(px(headline * 0.72 * 1.2))
                            .text_size(px(headline * 0.72))
                            .line_height(px(headline * 0.72 * 1.2))
                            .whitespace_nowrap()
                            .child(motion::rise(
                                tracked(&phrase, headline * 0.72, Some(gradient(&phrase, p))),
                                SharedString::from(format!("phrase-{idx}")),
                                Duration::ZERO,
                                if reduce { 0.0 } else { 20.0 },
                            )),
                        "welcome-h2",
                        Duration::from_millis(250),
                        24.0,
                    )),
            )
            .child(motion::rise(
                div()
                    .max_w(px(512.0))
                    .text_size(px(18.0))
                    .line_height(px(28.0))
                    .text_color(p.muted_foreground)
                    .child(t("connect.welcome.pitch")),
                "welcome-pitch",
                Duration::from_millis(400),
                16.0,
            ))
            .child(div().flex().flex_wrap().gap(px(8.0)).children(FEATURES.iter().enumerate().map(
                |(n, (glyph, text))| {
                    motion::rise(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded_full()
                            .border_1()
                            .border_color(p.border)
                            // The web's `bg-card/70 backdrop-blur`.
                            .bg(alpha(p.card, 0.7))
                            .backdrop_blur(px(8.0))
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .child(icon(glyph).size(px(16.0)).text_color(p.primary))
                            .child(t(text)),
                        SharedString::from(format!("feature-{n}")),
                        Duration::from_millis(500 + 60 * n as u64),
                        8.0,
                    )
                },
            )));
        let card = div()
            .w(px(502.0))
            .flex_none()
            .p(px(32.0))
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(mix(p.background, p.card, 0.85))
            .shadow(vec![gpui_kit::BoxShadow {
                color: p.primary.into(),
                offset: gpui_kit::point(px(0.0), px(30.0)),
                blur_radius: px(80.0),
                spread_radius: px(-40.0),
                inset: false,
            }])
            .child(
                div()
                    .mb(px(4.0))
                    .text_size(px(20.0))
                    .line_height(px(28.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(t("connect.welcome.connectTitle")),
            )
            .child(
                div()
                    .mb(px(20.0))
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(t("connect.welcome.connectHint")),
            )
            .child(form);
        div()
            .id("connect")
            .absolute()
            .inset_0()
            .occlude()
            .overflow_hidden()
            .bg(p.background)
            .child(bubbles(window, cx, p))
            .child(dot_grid(p))
            .child(petals(window, cx, p))
            .child(
                div().id("welcome-scroll").absolute().inset_0().overflow_y_scroll().child(
                    div().min_h_full().flex().items_center().justify_center().py(px(48.0)).px(px(16.0)).child(
                        div()
                            .w_full()
                            .max_w(px(1120.0))
                            .flex()
                            .items_center()
                            .gap(px(40.0))
                            .child(hero)
                            .child(motion::rise(card, "welcome-card", Duration::from_millis(300), 30.0)),
                    ),
                ),
            )
            .into_any_element()
    }
}

// ───────────────────────── Shared pieces ─────────────────────────

/// A form label: `font-bold`, 14px.
fn label(text: String) -> Div {
    div().text_size(px(14.0)).line_height(px(20.0)).font_weight(FontWeight::BOLD).child(text)
}

/// The web's `Input` at `h-11 rounded-xl text-base`: a border that turns the
/// primary color with a soft ring while focused, an icon inside on the left.
fn field(
    state: &Entity<InputState>,
    glyph: Option<&str>,
    invalid: bool,
    window: &Window,
    cx: &gpui_kit::App,
    p: &Palette,
) -> Div {
    let focused = state.read(cx).focus_handle(cx).is_focused(window);
    let input = gpui_kit::Styled::text_size(Input::new(state).appearance(false), px(16.0));
    let input = gpui_kit::Styled::flex_1(gpui_kit::Styled::px(input, px(0.0)));
    // `aria-invalid`: the border and ring turn the destructive color.
    let (edge, ring) = if invalid {
        (p.destructive.into(), alpha(p.destructive, 0.2))
    } else if focused {
        (Hsla::from(p.primary), alpha(p.primary, 0.5))
    } else {
        (Hsla::from(p.border), alpha(p.primary, 0.0))
    };
    div()
        .h(px(44.0))
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .rounded(radius_xl())
        .bg(p.card)
        .border_1()
        .border_color(edge)
        .shadow(if focused {
            vec![gpui_kit::BoxShadow {
                color: ring,
                offset: gpui_kit::point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(3.0),
                inset: false,
            }]
        } else {
            vec![gpui_kit::BoxShadow {
                color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.05),
                offset: gpui_kit::point(px(0.0), px(1.0)),
                blur_radius: px(2.0),
                spread_radius: px(0.0),
                inset: false,
            }]
        })
        .when_some(glyph, |el, g| el.child(icon(g).size(px(16.0)).text_color(p.muted_foreground)))
        .child(input)
}

fn error_text(text: String, centered: bool, p: &Palette) -> impl IntoElement {
    let mut text = text;
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    motion::rise(
        div()
            .when(centered, |el| el.text_center())
            .text_size(px(14.0))
            .line_height(px(20.0))
            .text_color(p.destructive)
            .child(text),
        "connect-error",
        Duration::ZERO,
        -6.0,
    )
}

/// The web's `btn h-11 rounded-xl font-bold` in the primary color, lifting on hover.
fn big_button(id: &'static str, text: String, p: &Palette) -> Stateful<Div> {
    let glow = gpui_kit::BoxShadow {
        color: p.primary.into(),
        offset: gpui_kit::point(px(0.0), px(8.0)),
        blur_radius: px(22.0),
        spread_radius: px(-8.0),
        inset: false,
    };
    div()
        .id(id)
        .relative()
        .w_full()
        .h(px(44.0))
        .flex()
        .flex_row_reverse()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .rounded(radius_xl())
        .bg(p.primary)
        .text_color(p.primary_foreground)
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        // The web's `.btn`: it lifts with a glow, and dips as it's pressed.
        .hover(move |s| s.translate_y(px(-2.0)).shadow(vec![glow.clone()]))
        .active(|s| s.translate_y(px(0.0)).scale(0.94))
        .child(text)
}

/// A provider's mark (Google, X, Twitch) in a color, or a key for one this app doesn't know.
fn provider_mark(id: &str, size: f32, color: Rgba) -> AnyElement {
    match id {
        "google" | "x" | "twitch" => gpui_kit::svg()
            .path(SharedString::from(format!("providers/{id}.svg")))
            .size(px(size))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        _ => icon("key-round").size(px(size)).text_color(color).into_any_element(),
    }
}

/// A "Continue with …" button with any mark in front.
fn provider_button_with(id: SharedString, mark: AnyElement, name: &str, p: &Palette) -> Stateful<Div> {
    let glow = gpui_kit::BoxShadow {
        color: p.primary.into(),
        offset: gpui_kit::point(px(0.0), px(14.0)),
        blur_radius: px(30.0),
        spread_radius: px(-16.0),
        inset: false,
    };
    div()
        .id(id)
        .relative()
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
        .shadow(vec![glow])
        .cursor_pointer()
        .group("provider")
        // `whileHover={{ y: -2 }} whileTap={{ scale: 0.97 }}`.
        .hover(|s| s.translate_y(px(-2.0)))
        .active(|s| s.scale(0.97))
        .child(div().id("provider-mark").group_hover("provider", |s| s.scale(1.1)).child(mark))
        .child(div().min_w_0().truncate().child(t_with("connect.provider.continueWith", &[("name", Arg::Str(name))])))
        .child(provider_arrow())
}

/// One "Continue with …" button: dark, the icon in the primary color, an arrow that nudges on hover.
fn provider_button(id: &'static str, glyph: &str, name: &str, p: &Palette) -> Stateful<Div> {
    let flower = glyph == "flower-2";
    let glow = gpui_kit::BoxShadow {
        color: p.primary.into(),
        offset: gpui_kit::point(px(0.0), px(14.0)),
        blur_radius: px(30.0),
        spread_radius: px(-16.0),
        inset: false,
    };
    div()
        .id(id)
        .relative()
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
        .shadow(vec![glow])
        .cursor_pointer()
        .group("provider")
        .hover(|s| s.translate_y(px(-2.0)))
        .active(|s| s.scale(0.97))
        .child(
            // The flower turns a petal's width and the building rises as they grow.
            div()
                .id("provider-mark")
                .group_hover("provider", move |s| {
                    let s = s.scale(1.1);
                    if flower { s.rotate(gpui_kit::radians(72f32.to_radians())) } else { s.translate_y(px(-2.0)) }
                })
                .child(icon(glyph).size(px(20.0)).text_color(p.primary)),
        )
        .child(div().min_w_0().truncate().child(t_with("connect.provider.continueWith", &[("name", Arg::Str(name))])))
        .child(provider_arrow())
}

/// The arrow on a "Continue with …" button, nudging along as it's hovered.
fn provider_arrow() -> impl IntoElement {
    div()
        .id("provider-arrow")
        .group_hover("provider", |s| s.translate_x(px(4.0)))
        .child(icon("arrow-right").size(px(16.0)))
}

fn closed_note(name: &str, p: &Palette) -> impl IntoElement {
    div()
        .text_center()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(p.muted_foreground)
        .child(t_with("connect.provider.closed", &[("name", Arg::Str(name))]))
}

/// The web's `size-9 rounded-full` ghost button with an icon.
fn round_button(id: &'static str, glyph: &str, p: &Palette) -> Stateful<Div> {
    let (bg, fg) = (p.muted, p.foreground);
    let lean = glyph == "arrow-left";
    div()
        .id(id)
        .size(px(36.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_color(p.muted_foreground)
        .cursor_pointer()
        // `hover:-translate-x-0.5`: a back arrow leans back.
        .hover(move |s| {
            let s = s.bg(bg).text_color(fg);
            if lean { s.translate_x(px(-2.0)) } else { s }
        })
        .child(icon(glyph).size(px(16.0)))
}

/// A turning loader, for a button that's waiting.
fn spinner(color: Rgba) -> impl IntoElement {
    gpui_kit::svg().path("icons/loader-circle.svg").size(px(16.0)).text_color(color).with_animation(
        "spin",
        Animation::new(Duration::from_millis(900)).repeat(),
        |el, t| el.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(t * std::f32::consts::TAU))),
    )
}

/// The web's `.gradient-text`: the primary sweeping toward a violet, a color per letter.
fn gradient(text: &str, p: &Palette) -> Vec<Hsla> {
    let to: Hsla = mix(p.primary, gpui_kit::rgb(0xa78bfa), 0.55);
    let from: Hsla = p.primary.into();
    let count = text.chars().count().max(2);
    (0..text.chars().count())
        .map(|n| {
            let k = n as f32 / (count - 1) as f32;
            Hsla {
                h: from.h + (to.h - from.h) * k,
                s: from.s + (to.s - from.s) * k,
                l: from.l + (to.l - from.l) * k,
                a: 1.0,
            }
        })
        .collect()
}

/// Big text set tight, as the web's `tracking-tight`.
fn tracked(text: &str, size: f32, colors: Option<Vec<Hsla>>) -> Div {
    let line = super::text::tracked(text.to_owned(), super::text::TIGHT);
    div().flex().flex_none().text_size(px(size)).child(match colors {
        Some(colors) => line.letter_colors(colors),
        None => line,
    })
}

/// Soft colored glows drifting behind the welcome (the web's bubble background at 30%,
/// blurred by 40px as its layer is).
fn bubbles(window: &mut Window, _cx: &mut Context<ConnectView>, p: &Palette) -> AnyElement {
    let blobs: [(Hsla, f32, f32, f32, u64); 5] = [
        (p.primary.into(), 0.55, 0.38, 520.0, 16000),
        (mix(p.primary, gpui_kit::rgb(0xa78bfa), 0.5), 0.3, 0.85, 560.0, 20000),
        (mix(p.primary, gpui_kit::rgb(0x7dd3fc), 0.45), 0.8, 0.8, 480.0, 24000),
        (p.primary.into(), 0.15, 0.2, 420.0, 18000),
        (mix(p.primary, gpui_kit::rgb(0xf9a8d4), 0.5), 0.72, 0.15, 460.0, 22000),
    ];
    let mut layer = div().absolute().inset_0();
    for (n, (color, x, y, size, period)) in blobs.into_iter().enumerate() {
        // The web's radial gradient, in a few steps the blur smooths over.
        let mut blob = div().absolute().left(gpui_kit::relative(x)).top(gpui_kit::relative(y)).size(px(0.0));
        for k in 0..5 {
            let d = size * (1.0 - k as f32 * 0.18);
            blob = blob.child(
                div()
                    .absolute()
                    .left(px(-d / 2.0))
                    .top(px(-d / 2.0))
                    .size(px(d))
                    .rounded_full()
                    .bg(Hsla { a: 0.044, ..color }),
            );
        }
        let phase = n as f32 * 1.3;
        layer = layer.child(motion::ambient(
            blob,
            SharedString::from(format!("bubble-{n}")),
            Duration::from_millis(period),
            window,
            move |el, t| {
                let a = t * std::f32::consts::TAU + phase;
                el.ml(px(a.cos() * 50.0)).mt(px(a.sin() * 40.0))
            },
        ));
    }
    layer.child(div().absolute().inset_0().backdrop_blur(px(40.0))).into_any_element()
}

/// The web's `.dot-grid`: a dot every 22px, fading toward the edges.
fn dot_grid(p: &Palette) -> impl IntoElement {
    let color = p.foreground;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let (cx, cy) = (w / 2.0, h / 2.0);
            let mut y = 11.0;
            while y < h {
                let mut x = 11.0;
                while x < w {
                    // radial-gradient(ellipse at center, black 30%, transparent 75%).
                    let d = (((x - cx) / cx).powi(2) + ((y - cy) / cy).powi(2)).sqrt() / std::f32::consts::SQRT_2;
                    let fade = 1.0 - ((d - 0.3) / 0.45).clamp(0.0, 1.0);
                    if fade > 0.0 {
                        let at = bounds.origin + gpui_kit::point(px(x - 1.0), px(y - 1.0));
                        window.paint_quad(
                            gpui_kit::fill(
                                gpui_kit::Bounds::new(at, gpui_kit::size(px(2.0), px(2.0))),
                                alpha(color, 0.12 * fade),
                            )
                            .corner_radii(px(1.0)),
                        );
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

/// A few petals drifting down (the web's `Petals`), unless motion is turned down.
fn petals(window: &mut Window, cx: &mut Context<ConnectView>, p: &Palette) -> AnyElement {
    if cx.reduce_motion() {
        return div().into_any_element();
    }
    const PETALS: [(f32, f32, f32); 6] = [
        (0.08, 0.0, 14.0),
        (0.22, 5.0, 18.0),
        (0.41, 2.0, 16.0),
        (0.63, 8.0, 20.0),
        (0.79, 3.0, 15.0),
        (0.92, 10.0, 19.0),
    ];
    let height = f32::from(window.viewport_size().height);
    let mut layer = div().absolute().inset_0();
    for (n, (left, delay, duration)) in PETALS.into_iter().enumerate() {
        let petal = div()
            .absolute()
            .left(gpui_kit::relative(left))
            .top(px(-32.0))
            .w(px(12.8))
            .h(px(9.6))
            .rounded_tl(px(10.0))
            .rounded_br(px(10.0))
            .bg(alpha(p.primary, 0.18));
        layer = layer.child(motion::ambient(
            petal,
            SharedString::from(format!("petal-{n}")),
            Duration::from_secs_f32(duration),
            window,
            move |el, t| {
                // Each starts `delay` seconds into its own fall, as the web's animation-delay.
                let k = (t + delay / duration).fract();
                el.mt(px(k * height * 1.1)).ml(px(-64.0 * k))
            },
        ));
    }
    layer.into_any_element()
}

/// Three dots taking turns, while the browser does its part.
fn dots(p: &Palette) -> impl IntoElement {
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

/// Whether the instance at this address is one Waifu Devs runs: one of its
/// own hosts, reached over https (the web's `lib/hosted.ts`). Only these link
/// Waifu Devs' terms and privacy policy.
fn hosted_by_us(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    parsed.scheme() == "https"
        && parsed.host_str().is_some_and(|h| HOSTED_DOMAINS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}"))))
}

/// A piece of "By continuing, you agree to the {terms} and the {privacy}.":
/// words (each with its space, so the line wraps between them) or a link.
#[derive(Debug, PartialEq)]
enum Piece {
    Word(String),
    Link(&'static str),
}

fn agreement_pieces(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut rest = text;
    loop {
        let next =
            ["terms", "privacy"].into_iter().filter_map(|k| rest.find(&format!("{{{k}}}")).map(|at| (at, k))).min();
        let (plain, link) = match next {
            Some((at, k)) => (&rest[..at], Some(k)),
            None => (rest, None),
        };
        pieces.extend(plain.split_inclusive(' ').map(|w| Piece::Word(w.to_owned())));
        match link {
            Some(k) => {
                pieces.push(Piece::Link(k));
                rest = &rest[plain.len() + k.len() + 2..];
            }
            None => return pieces,
        }
    }
}

/// "Any fuwa server works: ours, a friend's, or {link}." around its link.
fn split_link(text: &str) -> (String, String) {
    match text.split_once("{link}") {
        Some((a, b)) => (a.to_owned(), b.to_owned()),
        None => (text.to_owned(), String::new()),
    }
}

/// Whether an address is plain http somewhere other than this computer.
fn plain_http(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false };
    parsed.scheme() == "http"
        && !matches!(parsed.host(), Some(url::Host::Domain("localhost")))
        && !matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        && !matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
}

/// The issuer people know: "waifu.dev" for waifu.dev's, else its host (the web's `issuerName`).
fn issuer_name(issuer: &str) -> String {
    if issuer.is_empty() {
        return "waifu.dev".into();
    }
    let host =
        url::Url::parse(issuer).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| issuer.to_owned());
    // People know waifu.dev, not its API's address.
    host.strip_prefix("api.").map(str::to_owned).unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_footnote_splits_around_its_link() {
        assert_eq!(split_link("Any works, or {link}."), ("Any works, or ".into(), ".".into()));
        assert_eq!(issuer_name(""), "waifu.dev");
        assert_eq!(issuer_name("https://api.waifu.dev"), "waifu.dev");
        assert!(plain_http("http://chat.example.com") && !plain_http("http://127.0.0.1:8080"));
    }

    #[test]
    fn only_our_own_addresses_link_our_terms() {
        assert!(hosted_by_us("https://fuwa.chat") && hosted_by_us("https://eu.fuwa.chat/"));
        assert!(
            !hosted_by_us("http://fuwa.chat")
                && !hosted_by_us("https://notfuwa.chat")
                && !hosted_by_us("https://fuwa.chat.evil.example")
        );
        assert_eq!(
            agreement_pieces("By you, {terms} and {privacy}."),
            vec![
                Piece::Word("By ".into()),
                Piece::Word("you, ".into()),
                Piece::Link("terms"),
                Piece::Word(" ".into()),
                Piece::Word("and ".into()),
                Piece::Link("privacy"),
                Piece::Word(".".into()),
            ]
        );
    }
}
