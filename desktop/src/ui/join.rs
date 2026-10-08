//! Getting into servers beyond Browse: an invite's page (the web's
//! `pages/InvitePage.tsx`), applying to a server that lets people in by
//! hand (`join/ApplyDialog.tsx`), where an application stands
//! (`join/ApplicationStatus.tsx`), the applications waiting in the rail
//! (`join/Applied.tsx`), and making a server
//! (`dialogs/CreateServerDialog.tsx`).

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, BoxShadow, Context, Div, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, hsla, point, px,
};

use crate::core::api::Problem;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::join::{self, Applied, Found};
use crate::core::store::Connection;
use crate::pb;
use crate::ui::app::{Dialog, FuwaApp, Nav};
use crate::ui::context_menu::{Built, Item, run};
use crate::ui::instance_home::{
    capitalized, dot_grid, filled_button, hosted_by_us, hourglass, inline_markdown, outline_button, ringed_icon,
    server_door, shadow_xl, shake, shimmer, spinner, tag,
};
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, mix, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::{avatar, icon, pal, server_icon};

/// Half the parent's width, to center something absolute.
fn relative_half() -> gpui_kit::DefiniteLength {
    gpui_kit::relative(0.5)
}

/// As on the server.
const LINE_MAX: usize = 300;
const PARAGRAPH_MAX: usize = 1000;

// ───────────────────────── State ─────────────────────────

/// An invite being looked at.
pub struct InvitePage {
    /// The invite's instance, and the instance page it was opened from.
    pub key: String,
    pub on: String,
    pub code: String,
    /// Where to ask: the saved address of an instance we know, else the key's.
    address: String,
    /// Looking an invite up talks to its instance, which learns your
    /// address: one you never added asks first.
    confirmed: bool,
    pub found: Option<Result<Found, Problem>>,
    /// Sent to sign in to its instance, to join once back.
    pub signing_in: bool,
    /// Signed in for it: joins by itself once.
    auto: bool,
}

/// One question's answer box.
enum Field {
    Line(Entity<InputState>),
    Text(Entity<TextareaState>),
}

impl Field {
    fn value(&self, cx: &gpui_kit::App) -> String {
        match self {
            Field::Line(s) => s.read(cx).value().to_string(),
            Field::Text(s) => s.read(cx).value().to_string(),
        }
    }
}

/// Applying to a server.
pub struct ApplyForm {
    pub key: String,
    pub server: pb::Server,
    code: String,
    form: Option<Result<pb::JoinForm, String>>,
    fields: Vec<Field>,
    agreed: bool,
    missing: HashSet<usize>,
    sending: bool,
    error: Option<String>,
    sent: bool,
    nudges: u32,
    _subs: Vec<Subscription>,
}

/// Making a server.
pub struct CreateForm {
    name: Entity<InputState>,
    description: Entity<TextareaState>,
    discoverable: bool,
    /// An uploaded icon, on the instance it went to.
    icon: Option<(String, String)>,
    uploading: bool,
    /// Which instance it goes on, and the region picked there.
    place: String,
    region: Option<(String, String)>,
    busy: bool,
    error: Option<String>,
}

impl CreateForm {
    pub fn new(window: &mut Window, cx: &mut Context<FuwaApp>) -> (Self, Vec<Subscription>) {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("workspace.createServer.namePlaceholder"))
                .validate(|s, _| s.chars().count() <= 100)
        });
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 6)
                .placeholder(t("workspace.createServer.descriptionPlaceholder"))
        });
        let subs =
            vec![cx.subscribe_in(&name, window, |this: &mut FuwaApp, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.create_server_now(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            })];
        (
            CreateForm {
                name,
                description,
                discoverable: false,
                icon: None,
                uploading: false,
                place: String::new(),
                region: None,
                busy: false,
                error: None,
            },
            subs,
        )
    }
}

/// Seconds in their largest unit: "10 minutes", "1 hour", "7 days".
pub(crate) fn duration_words(seconds: i64) -> String {
    let (n, unit) = if seconds >= 86_400 && seconds % 86_400 == 0 || seconds >= 86_400 * 2 {
        (seconds / 86_400, "day")
    } else if seconds >= 3600 {
        (seconds / 3600, "hour")
    } else if seconds >= 60 {
        (seconds / 60, "minute")
    } else {
        (seconds.max(1), "second")
    };
    if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") }
}

/// How long ago, rounded down to its largest unit: "3 hours ago", "just now".
pub(crate) fn ago_words(ms: i64, now: i64) -> String {
    let minutes = (now - ms).max(0) / 60_000;
    let (n, unit) = match minutes {
        0 => return t("common.time.justNow"),
        m if m < 60 => (m, "minute"),
        m if m < 1440 => (m / 60, "hour"),
        m if m < 43_200 => (m / 1440, "day"),
        m if m < 525_600 => (m / 43_200, "month"),
        m => (m / 525_600, "year"),
    };
    if n == 1 { format!("1 {unit} ago") } else { format!("{n} {unit}s ago") }
}

fn now_ms() -> i64 {
    crate::core::dms::now_ms()
}

/// Where an application stands, as the card shows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Waiting,
    Accepted,
    Declined,
}

impl FuwaApp {
    // ───────────────────────── Invites ─────────────────────────

    /// An invite's page, on the instance it's for: there when that one's
    /// added here, else beside the one you're on, asking first.
    pub(crate) fn open_invite_page(
        &mut self,
        on: &str,
        key: &str,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let known = self.core.shared.read(|s| s.instance(key).map(|i| i.url.clone()));
        let page = InvitePage {
            key: key.to_owned(),
            on: on.to_owned(),
            code: code.to_owned(),
            address: known.clone().unwrap_or_else(|| join::address_of(key)),
            confirmed: known.is_some(),
            found: None,
            signing_in: false,
            auto: false,
        };
        if known.is_some() && on != key {
            self.navigate(Nav::Instance { key: key.to_owned() }, window, cx);
        }
        self.home.invite = Some(page);
        self.look_up_invite(cx);
        cx.notify();
    }

    fn look_up_invite(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.home.invite.as_mut().filter(|p| p.confirmed) else { return };
        page.found = None;
        let (core, address, code) = (self.core.clone(), page.address.clone(), page.code.clone());
        let (key, c) = (page.key.clone(), page.code.clone());
        self.run(cx, async move { core.look_up_invite(&address, &code).await }, move |this, result, cx| {
            if let Some(page) = this.home.invite.as_mut().filter(|p| p.key == key && p.code == c) {
                if let Ok(found) = &result {
                    this.home.known.insert(tag(&key, &found.server.id), found.server.clone());
                }
                page.found = Some(result);
            }
            cx.notify();
        });
    }

    /// Where an invite lands: the server it's for, who sent it, and one button to join.
    pub(crate) fn invite_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let Some(page) = self.home.invite.as_ref() else { return div().into_any_element() };
        let (key, code, confirmed) = (page.key.clone(), page.code.clone(), page.confirmed);
        let found = page.found.clone();
        let auto = page.auto;
        let (signed_out, ready) = self
            .core
            .shared
            .read(|s| {
                s.instance(&key).map(|i| {
                    let out = i.connection == Connection::SignedOut
                        || (i.me.is_none() && i.node.is_some() && i.connection != Connection::Connecting);
                    (out, i.me.is_some())
                })
            })
            .unwrap_or((true, false));
        let card: AnyElement = if !confirmed {
            self.elsewhere_card(&key, &p, window, cx)
        } else {
            match found {
                None => loading_card(&p, window),
                Some(Err(problem)) => self.broken_card(&problem, &p, cx),
                Some(Ok(found)) => {
                    // Signed in for this invite: join straight away, once.
                    if auto && ready && !signed_out {
                        if let Some(page) = self.home.invite.as_mut() {
                            page.auto = false;
                            page.signing_in = false;
                        }
                        let kind = self
                            .core
                            .shared
                            .read(|s| s.instance(&key).map(|i| join::kind_for(i, &found.server, false, false)));
                        if kind == Some(join::Kind::Join) {
                            let (server, c) = (found.server.clone(), code.clone());
                            let k = key.clone();
                            cx.defer_in(window, move |this, window, cx| this.join_now(&k, &server, &c, window, cx));
                        }
                    }
                    self.invite_card(&key, &code, &found, signed_out, ready, &p, window, cx)
                }
            }
        };
        // The web's `size-80 bg-primary/25 blur-3xl` light, breathing.
        let glow = motion::ambient(div().absolute().inset_0(), "invite-glow", Duration::from_secs(8), window, {
            let c = p.primary;
            move |el, t| {
                let k = 0.5 - 0.5 * (t * std::f32::consts::TAU).cos();
                let s = 320.0 * (1.0 + 0.12 * k);
                el.child(crate::ui::instance_home::soft_glow(
                    -s / 2.0,
                    -s / 2.0,
                    s,
                    s,
                    alpha(c, 0.25 * (0.5 + 0.2 * k)),
                    64.0,
                ))
            }
        });
        div()
            .id("invite-page")
            .relative()
            .size_full()
            .overflow_y_scroll()
            .child(dot_grid(0.6, p.foreground))
            .child(
                div()
                    .absolute()
                    .top(px(f32::from(window.viewport_size().height) * 0.25))
                    .left(relative_half())
                    .size_0()
                    .child(glow),
            )
            .child(div().relative().size_full().flex().items_center().justify_center().p(px(16.0)).child(card))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn invite_card(
        &mut self,
        key: &str,
        code: &str,
        found: &Found,
        signed_out: bool,
        ready: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server = &found.server;
        let w = 448.0;
        let banner = crate::ui::banner::server_banner_round(
            server,
            w - 2.0,
            144.0,
            Some(p.card.into()),
            f32::from(radius_3xl()) - 1.0,
            window,
            cx,
        );
        let accent = crate::ui::banner::accent(server);
        let inviter = found.inviter.clone().map(|u| {
            let name = crate::core::store::user_name(&u);
            let template = crate::ui::instance_home::template_text("workspace.invitePage.invitedYou");
            div().absolute().top(px(12.0)).left_0().right_0().flex().justify_center().child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(hsla(0.0, 0.0, 0.0, 0.35))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(hsla(0.0, 0.0, 1.0, 0.85))
                    .child(avatar(Some(&u), 20.0, p))
                    .child(crate::ui::text::hint_line(&template, &[("name", &name)], p)),
                "invite-from",
                Duration::from_millis(150),
                6.0,
            ))
        });
        let members = server.member_count;
        let streamer = self.prefs.streamer_mode;
        let address = if streamer { "•••••".to_owned() } else { key.replace('~', "/") };
        let on_line = crate::ui::text::hint_line(
            &crate::ui::instance_home::template_text("workspace.invitePage.on").replace("{address}", &address),
            &[("name", &found.node.name)],
            p,
        );
        let body = div()
            .relative()
            .mt(px(-48.0))
            .px(px(32.0))
            .pb(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .text_center()
            .child(motion::once(
                div().child(ringed_icon(server, 80.0, 25.6, 4.0, p).shadow(vec![BoxShadow {
                    color: alpha(accent.into(), 0.7),
                    offset: point(px(0.0), px(14.0)),
                    blur_radius: px(15.0),
                    spread_radius: px(-10.0),
                    inset: false,
                }])),
                SharedString::from(format!("invite-icon|{}", server.id)),
                Duration::from_millis(600),
                |el, t| {
                    let s = 1.0 - (1.0 - t).powi(3) * (1.0 + 2.6 * t);
                    el.relative().top(px(12.0 * (1.0 - s))).opacity(t.min(1.0))
                },
            ))
            .child(
                div()
                    .mt(px(4.0))
                    .text_size(px(24.0))
                    .line_height(px(32.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .child(server.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .items_center()
                    .gap_x(px(12.0))
                    .gap_y(px(4.0))
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .text_color(p.muted_foreground)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon("users").size(px(14.0)))
                            .child(t_with("workspace.invitePage.members", &[("count", Arg::Num(members))])),
                    )
                    .when(!found.channel_name.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.foreground)
                                .child(icon("hash").size(px(14.0)))
                                .child(found.channel_name.clone()),
                        )
                    }),
            )
            .when(!server.description.trim().is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .max_h(px(60.0))
                        .overflow_hidden()
                        .child(inline_markdown(&server.description, p.foreground)),
                )
            })
            .children(server_door(server, p))
            .when(!signed_out, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_center()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(on_line)
                        .when(hosted_by_us(&found.url), |el| el.child(icon("flower").size(px(14.0)))),
                )
            });
        let footer = {
            let inner: AnyElement = if signed_out {
                let url = found.url.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_center()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("workspace.invitePage.signIn")),
                    )
                    .child(
                        filled_button("invite-sign-in", 44.0, p.primary, p.primary_foreground, p)
                            .child(icon("log-in").size(px(16.0)))
                            .child(t("connect.account.signIn"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(page) = this.home.invite.as_mut() {
                                    page.signing_in = true;
                                    page.auto = true;
                                }
                                this.open_connect(true, window, cx);
                                if let Some(view) = &this.connect {
                                    view.update(cx, |view, cx| view.start_at(&url, window, cx));
                                }
                            })),
                    )
                    .into_any_element()
            } else if !ready {
                div()
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted_foreground)
                    .child(spinner(20.0, "invite-ready", window))
                    .into_any_element()
            } else {
                self.join_button(key, server, code, true, Some(t("workspace.invitePage.alreadyIn")), p, window, cx)
            };
            let expires = found.invite.expires_at.as_ref().map(|at| at.seconds * 1000).filter(|ms| *ms > 0).map(|ms| {
                let left = (ms - now_ms()).max(60_000) / 1000;
                div()
                    .mt(px(12.0))
                    .text_center()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(p.muted_foreground)
                    .child(t_with(
                        "workspace.invitePage.expiresIn",
                        &[("time", Arg::Str(&duration_words(round_left(left))))],
                    ))
            });
            div()
                .border_t_1()
                .border_color(p.border)
                .bg(alpha(p.background, 0.4))
                .rounded_b(px(f32::from(radius_3xl()) - 1.0))
                .p(px(32.0))
                .child(inner)
                .children(expires)
        };
        motion::rise(
            div()
                .w(px(w))
                .flex()
                .flex_col()
                .rounded(radius_3xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .shadow(shadow_2xl())
                .child(
                    div()
                        .relative()
                        .h(px(144.0))
                        .overflow_hidden()
                        .rounded_t(px(f32::from(radius_3xl()) - 1.0))
                        .child(banner)
                        .children(inviter),
                )
                .child(body)
                .child(footer),
            SharedString::from(format!("invite-card|{key}|{}", server.id)),
            Duration::ZERO,
            24.0,
        )
        .into_any_element()
    }

    /// Asks before opening an invite on an instance this app has never talked to.
    fn elsewhere_card(&mut self, key: &str, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let host = join::address_of(key);
        let globe = motion::ambient(
            icon("globe").size(px(28.0)),
            "elsewhere-globe",
            Duration::from_secs(18),
            window,
            |el, t| el.rotate(gpui_kit::radians(t * std::f32::consts::TAU)),
        );
        let about = crate::ui::text::hint_line(
            &crate::ui::instance_home::template_text("workspace.invitePage.elsewhere.about"),
            &[("host", &host)],
            p,
        );
        let actions = div()
            .mt(px(4.0))
            .w_full()
            .flex()
            .gap(px(8.0))
            .child(
                ghost_button("elsewhere-back", 36.0, p)
                    .flex_1()
                    .child(t("workspace.invitePage.elsewhere.back"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.home.invite = None;
                        cx.notify();
                    })),
            )
            .child(
                filled_button("elsewhere-go", 36.0, p.primary, p.primary_foreground, p)
                    .flex_1()
                    .child(t_with("workspace.invitePage.elsewhere.continue", &[("host", Arg::Str(&host))]))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(page) = this.home.invite.as_mut() {
                            page.confirmed = true;
                        }
                        this.look_up_invite(cx);
                        cx.notify();
                    })),
            );
        motion::rise(
            small_card(p)
                .child(round_badge(alpha(p.primary, 0.15), p.primary.into(), globe))
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child(t("workspace.invitePage.elsewhere.title")),
                )
                .child(div().text_sm().text_color(p.muted_foreground).child(about))
                .child(actions),
            SharedString::from(format!("elsewhere|{key}")),
            Duration::ZERO,
            16.0,
        )
        .into_any_element()
    }

    /// An invite that doesn't lead anywhere anymore, or an instance that can't be reached.
    fn broken_card(&mut self, problem: &Problem, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let gone = problem.code == tonic::Code::NotFound;
        let wiggle =
            motion::once(icon("link-2-off").size(px(28.0)), "broken-wiggle", Duration::from_millis(950), |el, t| {
                let k = ((t - 0.26) / 0.74).clamp(0.0, 1.0);
                let a = (k * std::f32::consts::TAU * 2.0).sin() * (1.0 - k) * 0.25;
                el.rotate(gpui_kit::radians(a))
            });
        motion::rise(
            small_card(p)
                .child(round_badge(p.muted.into(), p.muted_foreground.into(), wiggle))
                .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child(if gone {
                    t("workspace.invitePage.broken.gone")
                } else {
                    t("workspace.invitePage.broken.failed")
                }))
                .child(div().text_sm().text_color(p.muted_foreground).child(if gone {
                    t("workspace.invitePage.broken.goneAbout")
                } else {
                    capitalized(&problem.message)
                }))
                .child(
                    filled_button("broken-home", 36.0, p.primary, p.primary_foreground, p)
                        .w_auto()
                        .mt(px(4.0))
                        .child(t("workspace.invitePage.broken.home"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.home.invite = None;
                            cx.notify();
                        })),
                ),
            "broken-card",
            Duration::ZERO,
            16.0,
        )
        .into_any_element()
    }

    // ───────────────────────── Applying ─────────────────────────

    /// Opens the application for a server: its rules and questions, then a note that it went.
    pub(crate) fn open_apply(
        &mut self,
        key: &str,
        server: &pb::Server,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.home.apply = Some(ApplyForm {
            key: key.to_owned(),
            server: server.clone(),
            code: code.to_owned(),
            form: None,
            fields: Vec::new(),
            agreed: false,
            missing: HashSet::new(),
            sending: false,
            error: None,
            sent: false,
            nudges: 0,
            _subs: Vec::new(),
        });
        self.home.known.insert(tag(key, &server.id), server.clone());
        self.open_dialog(Dialog::Apply { key: key.to_owned(), server: server.id.clone() }, window, cx);
        self.load_join_form(window, cx);
    }

    fn load_join_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.home.apply.as_ref() else { return };
        let (core, key, sid, code) = (self.core.clone(), form.key.clone(), form.server.id.clone(), form.code.clone());
        self.run_in(
            window,
            cx,
            async move { core.join_form(&key, &sid, &code).await },
            move |this, result, window, cx| {
                let Some(apply) = this.home.apply.as_mut() else { return };
                match result {
                    Ok(form) => {
                        // Keep what they wrote for questions that are still asked.
                        let before: Vec<String> = apply.fields.iter().map(|f| f.value(cx)).collect();
                        let mut subs = Vec::new();
                        let mut fields = Vec::new();
                        for (n, q) in form.questions.iter().enumerate() {
                            let value = before.get(n).cloned().unwrap_or_default();
                            let field = if q.paragraph {
                                let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 8));
                                state.update(cx, |s, cx| s.set_value(value, window, cx));
                                subs.push(cx.subscribe_in(
                                    &state,
                                    window,
                                    |_: &mut FuwaApp, _, _: &InputEvent, _, cx| cx.notify(),
                                ));
                                Field::Text(state)
                            } else {
                                let state = cx.new(|cx| {
                                    InputState::new(window, cx).validate(|s, _| s.chars().count() <= LINE_MAX)
                                });
                                state.update(cx, |s, cx| s.set_value(value, window, cx));
                                subs.push(cx.subscribe_in(
                                    &state,
                                    window,
                                    |_: &mut FuwaApp, _, _: &InputEvent, _, cx| cx.notify(),
                                ));
                                Field::Line(state)
                            };
                            fields.push(field);
                        }
                        if let Some(apply) = this.home.apply.as_mut() {
                            apply.fields = fields;
                            apply._subs = subs;
                            apply.form = Some(Ok(form));
                        }
                    }
                    Err(err) => apply.form = Some(Err(err.message)),
                }
                cx.notify();
            },
        );
    }

    fn send_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(apply) = self.home.apply.as_mut() else { return };
        let Some(Ok(form)) = apply.form.clone() else { return };
        if apply.sending {
            return;
        }
        let answers: Vec<String> = apply.fields.iter().map(|f| f.value(cx).trim().to_owned()).collect();
        let empty: HashSet<usize> = form
            .questions
            .iter()
            .enumerate()
            .filter(|(n, q)| q.required && answers.get(*n).is_none_or(|a| a.is_empty()))
            .map(|(n, _)| n)
            .collect();
        let unagreed = !form.rules.is_empty() && !apply.agreed;
        apply.missing = empty.clone();
        if !empty.is_empty() || unagreed {
            apply.nudges += 1;
            cx.notify();
            return;
        }
        apply.sending = true;
        apply.error = None;
        let pairs: Vec<(String, String)> = form
            .questions
            .iter()
            .zip(answers)
            .map(|(q, a)| {
                let max = if q.paragraph { PARAGRAPH_MAX } else { LINE_MAX };
                (q.prompt.clone(), a.chars().take(max).collect())
            })
            .collect();
        let (core, key, server, code) =
            (self.core.clone(), apply.key.clone(), apply.server.clone(), apply.code.clone());
        cx.notify();
        self.run_in(
            window,
            cx,
            async move { core.apply_to_join(&key, &server, &code, pairs).await },
            |this, result, window, cx| {
                let Some(apply) = this.home.apply.as_mut() else { return };
                apply.sending = false;
                match result {
                    Ok(()) => apply.sent = true,
                    Err(err) => {
                        // The questions changed while they were answering: show the new ones.
                        let changed = err.message.contains("questions just changed");
                        apply.error = Some(err.message);
                        if changed {
                            this.load_join_form(window, cx);
                        }
                    }
                }
                cx.notify();
            },
        );
    }

    /// The dialogs about getting in, drawn on their own: making a server,
    /// applying, and where an application stands.
    pub(crate) fn render_join_dialog(
        &mut self,
        dialog: &Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = pal(cx);
        let (tag_name, body): (&str, AnyElement) = match dialog {
            Dialog::CreateServer { key } => ("create", self.create_server_body(key, &p, window, cx)),
            Dialog::Apply { key, server } => {
                let sent = self.home.apply.as_ref().is_some_and(|a| a.sent);
                if sent {
                    ("apply-sent", self.application_card(key, server, &p, window, cx))
                } else {
                    ("apply", self.apply_body(&p, window, cx))
                }
            }
            Dialog::Application { key, server } => ("application", self.application_card(key, server, &p, window, cx)),
            _ => return None,
        };
        let panel = div()
            .id("join-dialog-panel")
            .relative()
            .w(px(448.0))
            .max_h(window.viewport_size().height * 0.92)
            .overflow_y_scroll()
            .rounded(radius_3xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .shadow(shadow_2xl())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(body)
            .child(close_button(&p, cx));
        Some(
            motion::fade_in(
                div()
                    .id("join-dialog-scrim")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hsla(0.0, 0.0, 0.0, 0.5))
                    .occlude()
                    .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
                    .child(motion::rise(
                        panel,
                        SharedString::from(format!("join-dialog-{tag_name}")),
                        Duration::ZERO,
                        40.0,
                    )),
                SharedString::from(format!("join-dialog-fade-{tag_name}")),
                Duration::from_millis(200),
            )
            .into_any_element(),
        )
    }

    fn apply_body(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(apply) = self.home.apply.as_ref() else { return div().into_any_element() };
        let server = apply.server.clone();
        let hero = banner_hero(
            &server,
            &t("join.apply"),
            Some(icon("clipboard-pen").size(px(14.0)).into_any_element()),
            p,
            window,
            cx,
        );
        let mut body = div().flex().flex_col().gap(px(16.0)).px(px(24.0)).pb(px(24.0));
        body = body.child(
            div()
                .mt(px(-12.0))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(p.muted_foreground)
                .child(t("join.applyDialog.description")),
        );
        match &apply.form {
            Some(Err(problem)) => {
                body = body.child(div().text_sm().text_color(p.muted_foreground).child(capitalized(problem)));
            }
            None => {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(shimmer(48.0, radius_2xl(), p, window, 0))
                        .child(div().w(px(130.0)).child(shimmer(16.0, radius_lg(), p, window, 1)))
                        .child(shimmer(44.0, radius_xl(), p, window, 2)),
                );
            }
            Some(Ok(form)) => {
                let form = form.clone();
                if !form.rules.is_empty() {
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(section_label(&t("join.applyDialog.rules"), p))
                            .child(
                                div()
                                    .id("apply-rules")
                                    .max_h(px(224.0))
                                    .overflow_y_scroll()
                                    .child(rules_list(&form.rules, p)),
                            ),
                    );
                }
                if !form.questions.is_empty() {
                    let mut section =
                        div().flex().flex_col().gap(px(12.0)).child(section_label(&t("join.applyDialog.questions"), p));
                    for (n, q) in form.questions.iter().enumerate() {
                        section = section.child(self.question(n, q, p, window, cx));
                    }
                    body = body.child(section);
                }
                let apply = self.home.apply.as_ref().expect("checked above");
                let (agreed, sending, error, nudges) = (apply.agreed, apply.sending, apply.error.clone(), apply.nudges);
                let send = filled_button("apply-send", 44.0, p.primary, p.primary_foreground, p)
                    .when(sending, |el| el.opacity(0.5))
                    .child(if sending {
                        spinner(16.0, "apply-send", window)
                    } else {
                        icon("send").size(px(16.0)).into_any_element()
                    })
                    .child(t("join.applyDialog.send"))
                    .on_click(cx.listener(|this, _, window, cx| this.send_application(window, cx)));
                let mut end = div().flex().flex_col().gap(px(12.0));
                if !form.rules.is_empty() {
                    end = end.child(agree_check(agreed, &t("join.rules.agree"), p).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(a) = this.home.apply.as_mut() {
                                a.agreed = !a.agreed;
                            }
                            cx.notify();
                        },
                    )));
                }
                if let Some(error) = error {
                    end = end.child(div().text_sm().text_color(p.destructive).child(capitalized(&error)));
                }
                end = end.child(send);
                body = body.child(if nudges > 0 {
                    motion::once(
                        end,
                        SharedString::from(format!("apply-nudge-{nudges}")),
                        Duration::from_millis(400),
                        |el, t| el.relative().left(px(shake(t) * 1.33)),
                    )
                } else {
                    end.into_any_element()
                });
            }
        }
        div().flex().flex_col().gap(px(16.0)).child(hero).child(body).into_any_element()
    }

    fn question(
        &self,
        n: usize,
        q: &pb::JoinQuestion,
        p: &Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(apply) = self.home.apply.as_ref() else { return div().into_any_element() };
        let Some(field) = apply.fields.get(n) else { return div().into_any_element() };
        let value = field.value(cx);
        let max = if q.paragraph { PARAGRAPH_MAX } else { LINE_MAX };
        let bad = apply.missing.contains(&n) && value.trim().is_empty();
        let count = value.chars().count();
        let label = div()
            .flex()
            .items_baseline()
            .justify_between()
            .gap(px(8.0))
            .text_size(px(14.0))
            .line_height(px(20.0))
            .font_weight(FontWeight::BOLD)
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .child(q.prompt.clone())
                    .when(q.required, |el| el.child(div().text_color(p.destructive).child("\u{a0}*"))),
            )
            .when(!q.required, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p.muted_foreground)
                        .child(t("join.applyDialog.optional")),
                )
            });
        let focused = match field {
            Field::Line(s) => crate::ui::instance_home::has_focus(s, window, cx),
            Field::Text(s) => crate::ui::instance_home::has_focus(s, window, cx),
        };
        let input = div()
            .w_full()
            .px(px(12.0))
            .when(!q.paragraph, |el| el.h(px(44.0)).flex().items_center())
            .when(q.paragraph, |el| el.py(px(8.0)))
            .rounded(radius_xl())
            .border_1()
            .border_color(if bad { p.destructive } else { p.border })
            .text_size(px(14.0))
            .child(match field {
                Field::Line(s) => Input::new(s).appearance(false).small().into_any_element(),
                Field::Text(s) => Textarea::new(s).appearance(false).into_any_element(),
            });
        let input = crate::ui::instance_home::focus_ring(input, focused && !bad, p);
        let note = (bad || count + 100 > max).then(|| {
            motion::rise(
                div()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(if bad { p.destructive } else { p.muted_foreground })
                    .child(if bad {
                        t("join.applyDialog.needsAnswer")
                    } else {
                        t_with(
                            "join.applyDialog.charactersLeft",
                            &[("count", Arg::Num(max.saturating_sub(count) as i64))],
                        )
                    }),
                SharedString::from(format!("q-note-{n}-{bad}")),
                Duration::ZERO,
                -4.0,
            )
        });
        motion::rise(
            div().flex().flex_col().gap(px(6.0)).child(label).child(input).children(note),
            SharedString::from(format!("q-{n}-{}", q.prompt)),
            Duration::from_millis(50 * n as u64),
            8.0,
        )
        .into_any_element()
    }

    /// Your application to a server under its banner: where it stands, and
    /// what you can do about it.
    fn application_card(
        &mut self,
        key: &str,
        server_id: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (member, applied) = self
            .core
            .shared
            .read(|s| {
                s.instance(key)
                    .map(|i| (i.server(server_id).cloned(), i.applied.as_ref().and_then(|a| a.get(server_id)).cloned()))
            })
            .unwrap_or((None, None));
        let server = member
            .clone()
            .or_else(|| applied.as_ref().map(|a| a.server.clone()))
            .or_else(|| self.home.known.get(&tag(key, server_id)).cloned())
            .unwrap_or_default();
        let standing = if member.is_some() {
            Standing::Accepted
        } else if applied.as_ref().is_some_and(|a| a.status == pb::ApplicationStatus::Rejected) {
            Standing::Declined
        } else {
            Standing::Waiting
        };
        let applied_at = applied.as_ref().map(|a| a.applied_at);
        let reason = applied.as_ref().map(|a| a.reason.clone()).unwrap_or_default();
        let (eyebrow, badge) = match standing {
            Standing::Accepted => (t("join.card.eyebrowIn"), icon("check").size(px(14.0)).into_any_element()),
            Standing::Declined => (t("join.card.eyebrowDeclined"), icon("x").size(px(14.0)).into_any_element()),
            Standing::Waiting => (t("join.card.eyebrowWaiting"), hourglass(14.0, "card", window)),
        };
        let hero = banner_hero(&server, &eyebrow, Some(badge), p, window, cx);
        let timeline = timeline(standing, applied_at, &reason, &server, p, window);
        let id = tag(key, server_id);
        let withdrawing = self.home.withdrawing.contains(&id);
        let accent = crate::ui::banner::accent(&server);
        let buttons: AnyElement = match standing {
            Standing::Accepted => {
                let (k, s) = (key.to_owned(), server_id.to_owned());
                filled_button("card-open", 40.0, accent.into(), gpui_kit::rgb(0xffffff), p)
                    .child(icon("party-popper").size(px(16.0)))
                    .child(t_with("join.card.open", &[("server", Arg::Str(&server.name))]))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dialog = None;
                        this.open_joined(&k, &s, None, window, cx);
                    }))
                    .into_any_element()
            }
            Standing::Declined => {
                let (k, s, c) = (
                    key.to_owned(),
                    server.clone(),
                    applied.as_ref().map(|a| a.invite_code.clone()).unwrap_or_default(),
                );
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        outline_button("card-close", 40.0, p)
                            .flex_1()
                            .child(t("common.close"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .child(
                        filled_button("card-again", 40.0, p.primary, p.primary_foreground, p)
                            .flex_1()
                            .child(icon("clipboard-pen").size(px(16.0)))
                            .child(t("join.applyAgain"))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_apply(&k, &s, &c, window, cx))),
                    )
                    .into_any_element()
            }
            Standing::Waiting => {
                let (k, s) = (key.to_owned(), server.clone());
                let muted = p.muted_foreground;
                let red = p.destructive;
                let hover = p.accent;
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("card-take-back")
                            .flex_1()
                            .h(px(40.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(8.0))
                            .rounded(radius_xl())
                            .text_size(px(14.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(muted)
                            .cursor_pointer()
                            .when(withdrawing, |el| el.opacity(0.5))
                            .hover(move |st| st.bg(hover).text_color(red))
                            .child(icon("undo-2").size(px(16.0)))
                            .child(t("join.card.takeBack"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.withdraw(&k, &s, cx);
                                this.close_dialog(cx);
                            })),
                    )
                    .child(
                        filled_button("card-got-it", 40.0, p.primary, p.primary_foreground, p)
                            .flex_1()
                            .child(t("join.card.gotIt"))
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    )
                    .into_any_element()
            }
        };
        let tag_name = match standing {
            Standing::Accepted => "in",
            Standing::Declined => "no",
            Standing::Waiting => "wait",
        };
        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .pb(px(24.0))
            .child(hero)
            .child(div().px(px(24.0)).flex().flex_col().child(timeline))
            .child(div().px(px(24.0)).flex().flex_col().child(motion::rise(
                div().w_full().child(buttons),
                SharedString::from(format!("card-buttons-{tag_name}")),
                Duration::ZERO,
                4.0,
            )))
            .into_any_element()
    }

    // ───────────────────────── Making a server ─────────────────────────

    fn create_server_body(
        &mut self,
        key: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let places: Vec<(String, String, Option<pb::Node>, bool)> = self.core.shared.read(|s| {
            s.order
                .iter()
                .filter_map(|k| s.instance(k))
                .filter(|i| i.me.is_some())
                .map(|i| (i.key.clone(), i.name(), i.node.clone(), i.admin))
                .collect()
        });
        let form = &self.home.create;
        let place = places
            .iter()
            .find(|(k, ..)| *k == form.place)
            .or_else(|| places.iter().find(|(k, ..)| k == key))
            .or_else(|| places.first())
            .cloned();
        let Some((place_key, place_name, node, admin)) = place else { return div().into_any_element() };
        let creation = node.as_ref().map(|n| n.server_creation).unwrap_or_default();
        let blocked = creation == pb::ServerCreation::Disabled as i32
            || (creation == pb::ServerCreation::Admins as i32 && !admin);
        let regions = node.as_ref().map(|n| n.regions.clone()).unwrap_or_default();
        let picked = form
            .region
            .as_ref()
            .filter(|(k, _)| *k == place_key)
            .and_then(|(_, id)| regions.iter().find(|r| r.id == *id))
            .or_else(|| regions.iter().find(|r| r.home))
            .or_else(|| regions.first())
            .cloned();
        let name = form.name.read(cx).value().trim().to_owned();
        let icon_url = form.icon.as_ref().filter(|(k, _)| *k == place_key).map(|(_, u)| u.clone());
        let (busy, uploading, discoverable, error) = (form.busy, form.uploading, form.discoverable, form.error.clone());
        let preview = pb::Server {
            id: if name.is_empty() { "new".into() } else { name.clone() },
            name: if name.is_empty() { "?".into() } else { name.clone() },
            icon_url: icon_url.clone().unwrap_or_default(),
            ..Default::default()
        };
        let initial: String = name.chars().next().map(String::from).unwrap_or_default();
        let tile = div()
            .id("create-icon")
            .relative()
            .flex_none()
            .size(px(80.0))
            .cursor_pointer()
            .child(
                div()
                    .size(px(80.0))
                    .rounded(px(25.6))
                    .overflow_hidden()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.muted)
                    .child(motion::once(
                        server_icon(&preview, 78.0, 24.6, p).text_size(px(20.0)),
                        SharedString::from(format!("create-initial-{initial}")),
                        Duration::from_millis(300),
                        |el, t| el.opacity(0.6 + 0.4 * t),
                    )),
            )
            .when(uploading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(25.6))
                        .bg(hsla(0.0, 0.0, 0.0, 0.45))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(gpui_kit::white())
                        .child(spinner(20.0, "create-icon", window)),
                )
            })
            .child(
                div()
                    .absolute()
                    .right(px(-4.0))
                    .bottom(px(-4.0))
                    .size(px(28.0))
                    .rounded_full()
                    .border_2()
                    .border_color(p.card)
                    .bg(p.primary)
                    .text_color(p.primary_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("camera").size(px(14.0))),
            )
            .when(!blocked && !uploading, |el| {
                let k = place_key.clone();
                el.on_click(cx.listener(move |this, _, window, cx| this.pick_server_icon(&k, window, cx)))
            });
        let name_field = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(div().text_sm().font_weight(FontWeight::BOLD).child(t("workspace.createServer.name")))
            .child(crate::ui::instance_home::focus_ring(
                field_box(44.0, p).child(Input::new(&self.home.create.name).appearance(false).small()),
                crate::ui::instance_home::has_focus(&self.home.create.name, window, cx),
                p,
            ));
        let description = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .gap(px(4.0))
                    .text_sm()
                    .child(div().font_weight(FontWeight::BOLD).child(t("workspace.createServer.description")))
                    .child(div().text_color(p.muted_foreground).child(t("workspace.createServer.optional"))),
            )
            .child(crate::ui::instance_home::focus_ring(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(4.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(p.border)
                    .text_size(px(14.0))
                    .child(Textarea::new(&self.home.create.description).appearance(false)),
                crate::ui::instance_home::has_focus(&self.home.create.description, window, cx),
                p,
            ));
        let instances = (places.len() > 1).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(t("workspace.createServer.livesOn")))
                .child(div().flex().flex_wrap().gap(px(8.0)).children(places.iter().map(|(k, n, ..)| {
                    let on = *k == place_key;
                    let k = k.clone();
                    let hover = alpha(p.primary, 0.5);
                    div()
                        .id(SharedString::from(format!("lives-on|{k}")))
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { p.primary.into() } else { Hsla::from(p.border) })
                        .bg(if on { alpha(p.primary, 0.15) } else { hsla(0.0, 0.0, 0.0, 0.0) })
                        .text_color(if on { p.primary } else { p.foreground })
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .when(!on, |el| el.hover(move |s| s.border_color(hover)))
                        .child(n.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.home.create.place = k.clone();
                            this.home.create.region = None;
                            cx.notify();
                        }))
                })))
        });
        let region = (regions.len() > 1).then(|| {
            let home = regions
                .iter()
                .find(|r| r.home)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| t("workspace.createServer.homeRegion"));
            let picked_name = picked.as_ref().map(|r| r.name.clone()).unwrap_or_default();
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child(t("workspace.createServer.region")))
                .child(div().flex().flex_wrap().gap(px(8.0)).children(regions.iter().map(|r| {
                    let on = picked.as_ref().is_some_and(|x| x.id == r.id);
                    let (k, id) = (place_key.clone(), r.id.clone());
                    let mark: String = region_mark(&r.name);
                    let hover = alpha(p.primary, 0.5);
                    div()
                        .id(SharedString::from(format!("region|{}", r.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .py(px(6.0))
                        .pl(px(6.0))
                        .pr(px(12.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { p.primary.into() } else { Hsla::from(p.border) })
                        .bg(if on { alpha(p.primary, 0.15) } else { hsla(0.0, 0.0, 0.0, 0.0) })
                        .text_color(if on { p.primary } else { p.foreground })
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .when(!on, |el| el.hover(move |s| s.border_color(hover)))
                        .child(
                            div()
                                .size(px(24.0))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if on { p.primary } else { p.muted })
                                .text_color(if on { p.primary_foreground } else { p.muted_foreground })
                                .text_size(px(9.6))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(mark),
                        )
                        .child(r.name.clone())
                        .when(r.home, |el| {
                            el.child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(p.muted_foreground)
                                    .child(t("workspace.createServer.home")),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.home.create.region = Some((k.clone(), id.clone()));
                            cx.notify();
                        }))
                })))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(6.0))
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child(icon("map-pin").size(px(14.0)).text_color(p.primary))
                        .child(t_with(
                            "workspace.createServer.regionNote",
                            &[("region", Arg::Str(&picked_name)), ("home", Arg::Str(&home))],
                        )),
                )
        });
        let toggle = div()
            .id("create-browse")
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .p(px(12.0))
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(t("workspace.createServer.browse")))
                    .child(div().text_xs().text_color(p.muted_foreground).child(if discoverable {
                        t_with("workspace.createServer.browseOn", &[("instance", Arg::Str(&place_name))])
                    } else {
                        t("workspace.createServer.browseOff")
                    })),
            )
            .child(switch(discoverable, p, window, cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.home.create.discoverable = !this.home.create.discoverable;
                cx.notify();
            }));
        let problem = if blocked { Some(t("workspace.createServer.blocked")) } else { error };
        let can = !busy && !name.is_empty() && !blocked;
        let submit = filled_button("create-submit", 44.0, p.primary, p.primary_foreground, p)
            .when(!can, |el| el.opacity(0.5).cursor_default())
            .when(busy, |el| el.child(spinner(16.0, "create-submit", window)))
            .child(t("workspace.createServer.submit"))
            .when(can, |el| el.on_click(cx.listener(|this, _, window, cx| this.create_server_now(window, cx))));
        div()
            .p(px(24.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(20.0))
                    .pr(px(32.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .child(t("workspace.createServer.title")),
                    )
                    .child(
                        div()
                            .mt(px(4.0))
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(p.muted_foreground)
                            .child(t("workspace.createServer.about")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(div().flex().items_center().gap(px(16.0)).child(tile).child(name_field))
                    .child(description)
                    .children(instances)
                    .children(region)
                    .child(toggle)
                    .children(problem.map(|e| {
                        motion::rise(
                            div().text_sm().text_color(p.destructive).child(capitalized(&e)),
                            "create-error",
                            Duration::ZERO,
                            -4.0,
                        )
                    }))
                    .child(submit),
            )
            .into_any_element()
    }

    fn pick_server_icon(&mut self, key: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("desktop.account.choosePicture").into()),
        });
        let (core, key) = (self.core.clone(), key.to_owned());
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(kind) = crate::core::account::picture_type(&name) else {
                let _ = this.update(cx, |this, cx| {
                    this.home.create.error = Some(t("desktop.account.notAPicture"));
                    cx.notify();
                });
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.home.create.uploading = true;
                this.home.create.error = None;
                cx.notify();
            });
            let rx = core.spawn({
                let (core, key) = (core.clone(), key.clone());
                async move {
                    let bytes = crate::core::account::read_picture(&path).await?;
                    core.upload_picture(&key, pb::MediaPurpose::ServerIcon, kind, bytes).await
                }
            });
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.home.create.uploading = false;
                match result {
                    Ok(Ok(url)) => this.home.create.icon = Some((key, url)),
                    Ok(Err(err)) => this.home.create.error = Some(err.message),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn create_server_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Dialog::CreateServer { key }) = self.dialog.clone() else { return };
        let form = &self.home.create;
        if form.busy {
            return;
        }
        let place = if form.place.is_empty() { key } else { form.place.clone() };
        let name = form.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let description = form.description.read(cx).value().trim().to_owned();
        let regions = self
            .core
            .shared
            .read(|s| s.instance(&place).and_then(|i| i.node.as_ref().map(|n| n.regions.clone())))
            .unwrap_or_default();
        let region = form
            .region
            .as_ref()
            .filter(|(k, _)| *k == place)
            .and_then(|(_, id)| regions.iter().find(|r| r.id == *id))
            .filter(|r| regions.len() > 1 && !r.home)
            .map(|r| r.id.clone())
            .unwrap_or_default();
        let new = join::NewServer {
            name,
            description: description.chars().take(1000).collect(),
            discoverable: form.discoverable,
            icon_url: form.icon.as_ref().filter(|(k, _)| *k == place).map(|(_, u)| u.clone()).unwrap_or_default(),
            region,
        };
        self.home.create.busy = true;
        self.home.create.error = None;
        cx.notify();
        let core = self.core.clone();
        let k = place.clone();
        self.run_in(
            window,
            cx,
            async move { core.create_server_with(&k, new).await },
            move |this, result, window, cx| {
                this.home.create.busy = false;
                match result {
                    Ok(server) => {
                        this.dialog = None;
                        this.navigate(Nav::Server { key: place, server: server.id }, window, cx);
                    }
                    Err(err) => this.home.create.error = Some(err.message),
                }
                cx.notify();
            },
        );
    }

    /// Each opening of "Create a server" starts a blank form, on the instance it's opened for.
    pub(crate) fn reset_create_form(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let form = &mut self.home.create;
        form.place = key.to_owned();
        form.discoverable = false;
        form.icon = None;
        form.region = None;
        form.busy = false;
        form.error = None;
        form.name.update(cx, |s, cx| s.set_value("", window, cx));
        form.description.update(cx, |s, cx| s.set_value("", window, cx));
        // After the dialog takes the window's focus for itself.
        cx.defer_in(window, |this, window, cx| this.home.create.name.update(cx, |s, cx| s.focus(window, cx)));
    }

    // ───────────────────────── In the rail ─────────────────────────

    /// The servers you applied to on an instance, waiting in the rail.
    pub(crate) fn applied_buttons(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.core.load_applied(key);
        let list: Vec<Applied> = self.core.shared.read(|s| {
            s.instance(key)
                .map(|i| {
                    let mut list: Vec<Applied> = i
                        .applied
                        .as_ref()
                        .map(|a| a.values().filter(|a| i.server(&a.server.id).is_none()).cloned().collect())
                        .unwrap_or_default();
                    list.sort_by_key(|a| a.applied_at);
                    list
                })
                .unwrap_or_default()
        });
        let p = pal(cx);
        list.into_iter()
            .map(|a| {
                let waiting = a.waiting();
                let id = format!("applied|{key}|{}", a.server.id);
                let hovered = self.hovered.as_deref() == Some(id.as_str());
                let ring = if hovered { alpha(p.primary, 0.6) } else { alpha(p.muted_foreground, 0.4) };
                let badge_ring = mix(p.background, gpui_kit::rgb(0x000000), 0.25);
                let mark = div()
                    .absolute()
                    .right(px(-4.0))
                    .bottom(px(-4.0))
                    .size(px(20.0))
                    .rounded_full()
                    .border_3()
                    .border_color(badge_ring)
                    .bg(if waiting { gpui_kit::rgb(0xf59e0b) } else { p.destructive })
                    .text_color(gpui_kit::white())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if waiting {
                        hourglass(12.0, &id, window)
                    } else {
                        icon("x").size(px(12.0)).into_any_element()
                    });
                let (k, sid) = (key.to_owned(), a.server.id.clone());
                let face = div()
                    .id(SharedString::from(id.clone()))
                    .relative()
                    .size(px(48.0))
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(48.0))
                            .rounded_full()
                            .overflow_hidden()
                            .opacity(if hovered {
                                1.0
                            } else if waiting {
                                0.6
                            } else {
                                0.4
                            })
                            .child(server_icon(&a.server, 48.0, 24.0, &p)),
                    )
                    .child(div().absolute().inset_0().rounded_full().border_2().border_dashed().border_color(ring))
                    .child(mark)
                    .on_hover(cx.listener({
                        let id = id.clone();
                        move |this, on: &bool, _, cx| {
                            if *on {
                                this.hovered = Some(id.clone());
                            } else if this.hovered.as_deref() == Some(id.as_str()) {
                                this.hovered = None;
                            }
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let of = crate::ui::context_menu::MenuOf::Applied { key: k.clone(), server: sid.clone() };
                            this.open_context_menu(of, ev.position, window, cx);
                        }),
                    );
                motion::rise(
                    div().flex().w_full().justify_center().child(face),
                    SharedString::from(format!("{id}|in")),
                    Duration::ZERO,
                    10.0,
                )
                .into_any_element()
            })
            .collect()
    }

    /// The menu of a server you applied to: where it stands, take it back, apply again, or let it go.
    pub(crate) fn applied_items(&self, key: &str, server_id: &str) -> Built {
        let applied = self
            .core
            .shared
            .read(|s| s.instance(key).and_then(|i| i.applied.as_ref()).and_then(|a| a.get(server_id)).cloned());
        let Some(a) = applied else { return Built::of(Vec::new()) };
        let note = if a.waiting() {
            t_with("join.applied.waitingNote", &[("when", Arg::Str(&ago_words(a.applied_at, now_ms())))])
        } else if a.reason.is_empty() {
            t("join.applied.turnedDownNote")
        } else {
            t_with("join.applied.turnedDownBecause", &[("reason", Arg::Str(&a.reason))])
        };
        let (k, s) = (key.to_owned(), server_id.to_owned());
        let look = Item::act(t("join.seeWhereItStands"), "eye", {
            let (k, s) = (k.clone(), s.clone());
            run(move |this, window, cx| {
                this.open_dialog(Dialog::Application { key: k.clone(), server: s.clone() }, window, cx)
            })
        });
        // The web's menu label: the server, and where the application stands.
        let header = Item::act(
            format!("{} · {note}", a.server.name),
            if a.waiting() { "hourglass" } else { "x" },
            run(|_, _, _| {}),
        )
        .disabled(true);
        let rest = if a.waiting() {
            let server = a.server.clone();
            vec![
                Item::act(t("join.applied.takeBack"), "undo-2", {
                    let k = k.clone();
                    run(move |this, _, cx| this.withdraw(&k, &server, cx))
                })
                .danger(),
            ]
        } else {
            let (server, code) = (a.server.clone(), a.invite_code.clone());
            vec![
                Item::act(t("join.applyAgain"), "clipboard-pen", {
                    let k = k.clone();
                    run(move |this, window, cx| this.open_apply(&k, &server, &code, window, cx))
                }),
                Item::act(t("join.applied.remove"), "x", {
                    run(move |this, _, cx| {
                        this.core.set_applied(&k, &s, None);
                        cx.notify();
                    })
                }),
            ]
        };
        Built::of(vec![vec![header], vec![look], rest])
    }
}

// ───────────────────────── Pieces ─────────────────────────

/// "Europe" → "EU": a region's little mark (the web's `regionMark`).
fn region_mark(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = words.first().copied().unwrap_or("?");
    if (2..=3).contains(&first.len()) && first.chars().all(|c| c.is_ascii_uppercase()) {
        return first.to_owned();
    }
    let out: String = if words.len() > 1 {
        words.iter().filter_map(|w| w.chars().next()).take(2).collect()
    } else {
        first.chars().take(2).collect()
    };
    out.to_uppercase()
}

/// Seconds left on an invite, rounded in its largest unit (the web's `timeLeft`).
fn round_left(seconds: i64) -> i64 {
    let minutes = (seconds as f64 / 60.0).round().max(1.0) as i64;
    if minutes >= 1440 {
        ((minutes as f64 / 1440.0).round() as i64) * 86_400
    } else if minutes >= 60 {
        ((minutes as f64 / 60.0).round() as i64) * 3600
    } else {
        minutes * 60
    }
}

/// The web's `shadow-2xl`.
fn shadow_2xl() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0.0, 0.0, 0.0, 0.25),
        offset: point(px(0.0), px(25.0)),
        blur_radius: px(25.0),
        spread_radius: px(-12.0),
        inset: false,
    }]
}

/// The dialog's close button, at its top right.
fn close_button(p: &Palette, cx: &mut Context<FuwaApp>) -> impl IntoElement {
    let (bg, fg) = (p.muted, p.foreground);
    div()
        .id("join-dialog-close")
        .absolute()
        .top(px(16.0))
        .right(px(16.0))
        .size(px(32.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(p.muted_foreground)
        .cursor_pointer()
        .hover(move |s| s.bg(bg).text_color(fg))
        .child(icon("x").size(px(16.0)))
        .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx)))
}

/// A bordered field (`h-11 rounded-xl border`) for an input inside.
fn field_box(tall: f32, p: &Palette) -> Div {
    div()
        .w_full()
        .h(px(tall))
        .px(px(12.0))
        .flex()
        .items_center()
        .rounded(radius_xl())
        .border_1()
        .border_color(p.border)
        .text_size(px(14.0))
}

/// The web's `variant="ghost"` button.
fn ghost_button(id: &str, tall: f32, p: &Palette) -> Stateful<Div> {
    let hover = p.accent;
    div()
        .id(SharedString::from(id.to_owned()))
        .h(px(tall))
        .px(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_xl())
        .text_size(px(14.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
}

/// The cards of the invite page's other states (`max-w-md rounded-3xl p-8 text-center shadow-xl`).
fn small_card(p: &Palette) -> Div {
    div()
        .w(px(448.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(12.0))
        .p(px(32.0))
        .rounded(radius_3xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .text_center()
        .shadow(shadow_xl(p))
}

fn round_badge(bg: Hsla, fg: Hsla, child: AnyElement) -> Div {
    div().size(px(64.0)).rounded_full().flex().items_center().justify_center().bg(bg).text_color(fg).child(child)
}

/// The invite page while it looks the invite up.
fn loading_card(p: &Palette, window: &Window) -> AnyElement {
    div()
        .w(px(448.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(12.0))
        .p(px(32.0))
        .rounded(radius_3xl())
        .border_1()
        .border_color(p.border)
        .bg(p.card)
        .shadow(shadow_xl(p))
        .child(div().size(px(80.0)).child(shimmer(80.0, radius_3xl(), p, window, 0)))
        .child(div().w(px(192.0)).child(shimmer(24.0, radius_lg(), p, window, 1)))
        .child(div().w(px(128.0)).child(shimmer(16.0, radius_lg(), p, window, 2)))
        .child(div().mt(px(16.0)).w_full().child(shimmer(44.0, radius_xl(), p, window, 3)))
        .into_any_element()
}

/// "RULES", "A FEW QUESTIONS".
fn section_label(text: &str, p: &Palette) -> Div {
    div()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .font_weight(FontWeight::BOLD)
        .text_color(p.muted_foreground)
        .child(text.to_uppercase())
}

/// A server's rules, numbered (the web's `RulesList`).
pub(crate) fn rules_list(rules: &[String], p: &Palette) -> Div {
    div().flex().flex_col().gap(px(8.0)).children(rules.iter().enumerate().map(|(n, rule)| {
        motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .bg(alpha(p.muted, 0.5))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .child(
                    div()
                        .size(px(24.0))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(alpha(p.primary, 0.15))
                        .text_color(p.primary)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .child((n + 1).to_string()),
                )
                .child(div().flex_1().min_w_0().pt(px(2.0)).child(inline_markdown(rule, p.foreground))),
            SharedString::from(format!("rule-{n}-{rule}")),
            Duration::from_millis(45 * n.min(10) as u64),
            0.0,
        )
    }))
}

/// "I agree": a card that fills in and draws its tick when pressed (the web's `AgreeCheck`).
pub(crate) fn agree_check(checked: bool, label: &str, p: &Palette) -> Stateful<Div> {
    let hover = alpha(p.primary, 0.4);
    div()
        .id("apply-agree")
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.0))
        .p(px(12.0))
        .rounded(radius_2xl())
        .border_1()
        .border_color(if checked { alpha(p.primary, 0.6) } else { Hsla::from(p.border) })
        .bg(if checked { alpha(p.primary, 0.1) } else { hsla(0.0, 0.0, 0.0, 0.0) })
        .text_size(px(14.0))
        .line_height(px(20.0))
        .font_weight(FontWeight::BOLD)
        .cursor_pointer()
        .when(!checked, |el| el.hover(move |s| s.border_color(hover)))
        .child(motion::once(
            div()
                .size(px(24.0))
                .flex_none()
                .rounded(radius_lg())
                .border_2()
                .border_color(if checked { p.primary.into() } else { alpha(p.muted_foreground, 0.4) })
                .bg(if checked { p.primary.into() } else { hsla(0.0, 0.0, 0.0, 0.0) })
                .text_color(p.primary_foreground)
                .flex()
                .items_center()
                .justify_center()
                .when(checked, |el| el.child(icon("check").size(px(16.0)))),
            SharedString::from(format!("agree-{checked}")),
            Duration::from_millis(300),
            move |el, t| {
                let s = if checked { 1.0 + 0.25 * (t * std::f32::consts::PI).sin() } else { 1.0 };
                el.size(px(24.0 * s)).m(px(-12.0 * (s - 1.0)))
            },
        ))
        .child(div().flex_1().min_w_0().child(label.to_owned()))
}

/// A switch (the web's `Switch`): a pill whose knob slides over.
fn switch(on: bool, p: &Palette, window: &mut Window, cx: &mut Context<FuwaApp>) -> impl IntoElement {
    let x = motion::follow("create-switch", if on { 14.0 } else { 0.0 }, window, cx);
    div()
        .w(px(32.0))
        .h(px(18.4))
        .flex_none()
        .rounded_full()
        .bg(if on { p.primary } else { mix(p.muted, p.foreground, 0.08).into() })
        .p(px(1.0))
        .child(div().relative().left(px(x)).size(px(16.4)).rounded_full().bg(if on {
            p.primary_foreground
        } else {
            p.background
        }))
}

/// The top of the applying and application screens: the banner, the server's
/// icon over its bottom edge with a little sign on it, a line above the name,
/// the name, and how many are in it (the web's `BannerHero`, `bleed`).
fn banner_hero(
    server: &pb::Server,
    eyebrow: &str,
    badge: Option<AnyElement>,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<FuwaApp>,
) -> AnyElement {
    let accent = crate::ui::banner::accent(server);
    let banner = crate::ui::banner::server_banner_round(
        server,
        446.0,
        160.0,
        Some(p.card.into()),
        f32::from(radius_3xl()) - 1.0,
        window,
        cx,
    );
    let members = server.member_count;
    let eyebrow_color = mix(accent.into(), p.foreground, 0.25);
    let green: Hsla = gpui_kit::rgb(0x10b981).into();
    let ping = motion::ambient(
        div().absolute().rounded_full().bg(alpha(green.into(), 0.6)),
        "hero-ping",
        Duration::from_millis(1000),
        window,
        |el, t| {
            let s = 8.0 * (1.0 + t);
            el.size(px(s)).left(px((14.0 - s) / 2.0)).top(px((14.0 - s) / 2.0)).opacity(1.0 - t)
        },
    );
    let icon_box = div()
        .relative()
        .w(px(88.0))
        .child(ringed_icon(server, 80.0, 25.6, 4.0, p).shadow(shadow_xl(p)))
        .when_some(badge, |el, badge| {
            el.child(
                div()
                    .absolute()
                    .right(px(-8.0 + 4.0))
                    .bottom(px(-4.0 + 4.0))
                    .size(px(34.0))
                    .rounded_full()
                    .bg(p.card)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(28.0))
                            .rounded_full()
                            .bg(accent)
                            .text_color(gpui_kit::white())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(badge),
                    ),
            )
        });
    div()
        .flex()
        .flex_col()
        .child(div().w_full().h(px(160.0)).overflow_hidden().rounded_t(px(f32::from(radius_3xl()) - 1.0)).child(banner))
        .child(
            div()
                .relative()
                .mt(px(-44.0))
                .px(px(24.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(motion::once(
                    icon_box,
                    SharedString::from(format!("hero-icon|{}", server.id)),
                    Duration::from_millis(520),
                    |el, t| {
                        let s = 1.0 - (1.0 - t).powi(3) * (1.0 + 2.6 * t);
                        el.relative().top(px(14.0 * (1.0 - s))).opacity(t.min(1.0))
                    },
                ))
                .child(
                    div()
                        .pr(px(32.0))
                        .flex()
                        .flex_col()
                        .when(!eyebrow.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .text_color(eyebrow_color)
                                    .child(eyebrow.to_uppercase()),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(24.0))
                                .line_height(px(32.0))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .child(server.name.clone()),
                        )
                        .when(members > 0, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_size(px(12.0))
                                    .line_height(px(16.0))
                                    .text_color(p.muted_foreground)
                                    .child(
                                        div()
                                            .relative()
                                            .size(px(14.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(ping)
                                            .child(div().size(px(6.0)).rounded_full().bg(green)),
                                    )
                                    .child(icon("users").ml(px(2.0)).size(px(14.0)))
                                    .child(t_with("join.banner.members", &[("count", Arg::Num(members))])),
                            )
                        }),
                ),
        )
        .into_any_element()
}

/// Where an application stands, as three stops on a line: sent, being read,
/// and the answer (the web's `ApplicationTimeline`).
fn timeline(
    standing: Standing,
    applied_at: Option<i64>,
    reason: &str,
    server: &pb::Server,
    p: &Palette,
    window: &Window,
) -> AnyElement {
    let accent: Hsla = crate::ui::banner::accent(server);
    let done = standing != Standing::Waiting;
    let amber: Hsla = gpui_kit::rgb(0xf59e0b).into();
    struct Stop {
        glyph: AnyElement,
        title: String,
        note: String,
        bg: Hsla,
        fg: Hsla,
        later: bool,
        now: bool,
        no: bool,
    }
    let white: Hsla = gpui_kit::white();
    let stops = vec![
        Stop {
            glyph: icon("send").size(px(14.0)).into_any_element(),
            title: t("join.timeline.sent"),
            note: match applied_at {
                Some(at) => t_with("join.timeline.appliedAgo", &[("when", Arg::Str(&ago_words(at, now_ms())))]),
                None => t("join.timeline.applied"),
            },
            bg: accent,
            fg: white,
            later: false,
            now: false,
            no: false,
        },
        Stop {
            glyph: if done {
                icon("eye").size(px(14.0)).into_any_element()
            } else {
                hourglass(14.0, "timeline", window)
            },
            title: if done { t("join.timeline.read") } else { t("join.timeline.beingRead") },
            note: if done { t("join.timeline.readNote") } else { t("join.timeline.beingReadNote") },
            bg: if done { accent } else { amber },
            fg: white,
            later: false,
            now: !done,
            no: false,
        },
        match standing {
            Standing::Declined => Stop {
                glyph: icon("x").size(px(14.0)).into_any_element(),
                title: t("join.timeline.turnedDown"),
                note: if reason.is_empty() { t("join.timeline.noReason") } else { String::new() },
                bg: p.destructive.into(),
                fg: white,
                later: false,
                now: false,
                no: true,
            },
            Standing::Accepted => Stop {
                glyph: icon("party-popper").size(px(14.0)).into_any_element(),
                title: t("join.timeline.youreIn"),
                note: t("join.timeline.inYourList"),
                bg: accent,
                fg: white,
                later: false,
                now: false,
                no: false,
            },
            Standing::Waiting => Stop {
                glyph: icon("check").size(px(14.0)).into_any_element(),
                title: t("join.timeline.letIn"),
                note: t("join.timeline.letInNote"),
                bg: p.muted.into(),
                fg: p.muted_foreground.into(),
                later: true,
                now: false,
                no: false,
            },
        },
    ];
    let line_color = if standing == Standing::Declined { alpha(p.destructive, 0.6) } else { accent };
    let fill_to = if standing == Standing::Waiting { 0.5 } else { 1.0 };
    let card = p.card;
    let rows: Vec<AnyElement> = stops
        .into_iter()
        .enumerate()
        .map(|(n, s)| {
            let ping = s.now.then(|| {
                motion::ambient(
                    div().absolute().rounded_full().bg(Hsla { a: 0.5, ..amber }),
                    "timeline-ping",
                    Duration::from_millis(1000),
                    window,
                    |el, t| {
                        let sz = 24.0 * (1.0 + t);
                        el.size(px(sz)).left(px((24.0 - sz) / 2.0)).top(px((24.0 - sz) / 2.0)).opacity(1.0 - t)
                    },
                )
            });
            motion::rise(
                div()
                    .relative()
                    .flex()
                    .items_start()
                    .gap(px(12.0))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .size(px(32.0))
                            .m(px(-4.0))
                            .rounded_full()
                            .bg(card)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .relative()
                                    .size(px(24.0))
                                    .rounded_full()
                                    .bg(s.bg)
                                    .text_color(s.fg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .children(ping)
                                    .child(s.glyph),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pt(px(2.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .when(s.later, |el| el.text_color(p.muted_foreground))
                                    .child(s.title),
                            )
                            .when(!s.note.is_empty(), |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(p.muted_foreground)
                                        .child(s.note),
                                )
                            })
                            .when(s.no && !reason.is_empty(), |el| {
                                el.child(
                                    div()
                                        .mt(px(6.0))
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .rounded(radius_xl())
                                        .border_l_4()
                                        .border_color(alpha(p.destructive, 0.6))
                                        .bg(alpha(p.destructive, 0.05))
                                        .text_sm()
                                        .child(t_with("join.quoted", &[("text", Arg::Str(reason))])),
                                )
                            }),
                    ),
                SharedString::from(format!("stop-{n}")),
                Duration::from_millis(100 + 80 * n as u64),
                0.0,
            )
            .into_any_element()
        })
        .collect();
    let track = div().absolute().left(px(11.0)).top(px(12.0)).bottom(px(12.0)).w(px(2.0)).rounded_full().bg(p.muted);
    let filled = motion::once(
        div().absolute().left(px(11.0)).top(px(12.0)).w(px(2.0)).rounded_full().bg(line_color),
        SharedString::from(format!("timeline-fill-{fill_to}")),
        Duration::from_millis(600),
        move |el, t| {
            let e = 1.0 - (1.0 - t).powi(3);
            // The line is a share of the list's height; 112 is three stops' worth.
            el.h(px(112.0 * fill_to * e))
        },
    );
    div()
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(track)
        .child(filled)
        .children(rows)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_in_their_largest_unit() {
        assert_eq!(duration_words(600), "10 minutes");
        assert_eq!(duration_words(3600), "1 hour");
        assert_eq!(duration_words(86_400), "1 day");
        assert_eq!(duration_words(7 * 86_400), "7 days");
    }

    #[test]
    fn invites_round_what_is_left() {
        assert_eq!(round_left(6 * 86_400 + 23 * 3600), 7 * 86_400);
        assert_eq!(round_left(5400), 2 * 3600);
        assert_eq!(round_left(10), 60);
    }

    #[test]
    fn regions_get_two_letters() {
        assert_eq!(region_mark("Europe"), "EU");
        assert_eq!(region_mark("US West"), "US");
        assert_eq!(region_mark("North America"), "NA");
    }
}
