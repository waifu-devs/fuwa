//! The app's settings, full screen with a side menu, as on the web. These
//! are this computer's, for every instance: how fuwa looks and moves,
//! notifications, streamer mode, the accounts signed in here, and whether
//! anonymous reports help fix bugs.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, EventEmitter, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::core::Core;
use crate::core::config::{MotionChoice, NotifyFor, Prefs};
use crate::core::store::{Connection, user_name};
use crate::ui::motion;
use crate::ui::settings_account::AccountForm;
use crate::ui::settings_look::{Look, sync_sliders};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{avatar, conn_dot, icon, icon_button, pal, primary_button, soft_button};

pub enum SettingsEvent {
    Close,
    Prefs,
    SignIn { key: String },
    AddInstance,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Profile,
    Security,
    Appearance,
    Background,
    Motion,
    Notifications,
    Streamer,
    Accounts,
    Keyboard,
    Privacy,
    Updates,
    About,
}

/// Your account's pages, then the app's.
const ACCOUNT_PAGES: [(Page, &str, &str); 2] =
    [(Page::Profile, "user-round-pen", "Profile"), (Page::Security, "key-round", "Password and devices")];

const PAGES: [(Page, &str, &str); 10] = [
    (Page::Appearance, "palette", "Appearance"),
    (Page::Background, "image", "Background"),
    (Page::Motion, "sparkles", "Motion"),
    (Page::Notifications, "bell", "Notifications"),
    (Page::Streamer, "eye-off", "Streamer mode"),
    (Page::Accounts, "user", "Accounts"),
    (Page::Keyboard, "keyboard", "Keyboard"),
    (Page::Privacy, "shield-check", "Privacy"),
    (Page::Updates, "refresh-cw", "Updates"),
    (Page::About, "info", "About"),
];

pub struct SettingsView {
    pub(crate) core: Arc<Core>,
    pub(crate) page: Page,
    pub(crate) keys: crate::ui::settings_keys::Keys,
    pub(crate) account: AccountForm,
    pub(crate) look: Look,
    /// The reports' counts last shown on the Privacy page.
    pub(crate) pending: crate::core::reports::Pending,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let look = Look::new(&core.prefs(), window, cx);
        // The Privacy page's counts change without the store changing: look again now and then.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(600)).await;
                let looked = this.update(cx, |this, cx| {
                    if this.page == Page::Privacy && crate::core::reports::pending() != this.pending {
                        cx.notify();
                    }
                });
                if looked.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            core,
            page: Page::Appearance,
            keys: Default::default(),
            account: AccountForm::new(window, cx),
            look,
            pending: Default::default(),
        }
    }

    pub(crate) fn set(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Prefs)) {
        self.core.set_prefs(f);
        cx.emit(SettingsEvent::Prefs);
        cx.notify();
    }

    fn page(
        &mut self,
        prefs: &Prefs,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (String, String, AnyElement) {
        match self.page {
            Page::Profile => {
                ("Your profile".into(), "How people see you on this instance.".into(), self.profile_page(p, window, cx))
            }
            Page::Security => {
                ("Password and devices".into(), "Keep your account yours.".into(), self.security_page(p, window, cx))
            }
            Page::Appearance => (
                "Appearance".into(),
                "How fuwa looks on this computer, the same themes as everywhere else.".into(),
                self.appearance_page(prefs, p, window, cx),
            ),
            Page::Background => (
                "Background".into(),
                "A picture and an effect behind the app, under any theme without its own.".into(),
                self.background_page(prefs, p, window, cx),
            ),
            Page::Motion => {
                let reduced = cx.reduce_motion();
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(28.0))
                    .child(section(
                        "Animations",
                        segmented(
                            "motion",
                            vec![
                                ("Like my computer", prefs.motion == MotionChoice::System),
                                ("Full", prefs.motion == MotionChoice::Full),
                                ("Reduced", prefs.motion == MotionChoice::Reduced),
                            ],
                            p,
                            window,
                            cx,
                            |this, n, cx| {
                                let m = [MotionChoice::System, MotionChoice::Full, MotionChoice::Reduced][n];
                                this.set(cx, |pr| pr.motion = m);
                            },
                        ),
                        p,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.0))
                            .p(px(18.0))
                            .rounded(corner(16.0))
                            .bg(p.secondary)
                            .child(bouncer(p, reduced))
                            .child(div().text_sm().text_color(p.muted_foreground).child(if reduced {
                                "Things appear in place, without moving. Nothing bounces or slides."
                            } else {
                                "Things spring into place, slide and glide. Reduced keeps them still."
                            })),
                    );
                ("Motion".into(), "How much things move.".into(), body.into_any_element())
            }
            Page::Notifications => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(28.0))
                    .child(toggle_row(
                        "notify",
                        "Notifications",
                        "For messages that arrive while you're looking elsewhere: a card in the corner, or your computer's own notification while fuwa is in the background.",
                        prefs.notifications,
                        p,
                        cx,
                        |this, on, cx| this.set(cx, |pr| pr.notifications = on),
                    ))
                    .child(section(
                        "Notify me about",
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(segmented(
                                "notify-for",
                                vec![
                                    ("Only @mentions", prefs.notify_for == NotifyFor::Mentions),
                                    ("All messages", prefs.notify_for == NotifyFor::All),
                                ],
                                p,
                                window,
                                cx,
                                |this, n, cx| {
                                    let v = if n == 0 { NotifyFor::Mentions } else { NotifyFor::All };
                                    this.set(cx, |pr| pr.notify_for = v)
                                },
                            ))
                            .child(div().text_sm().text_color(p.muted_foreground).child(
                                "In servers and channels you haven't set yourself, from their bell. Private messages always notify you.",
                            )),
                        p,
                    ))
                    .child(div().child(
                        soft_button("notify-test", "Send a test notification", p)
                            .child(icon("bell-ring").size(px(16.0)))
                            .on_click(|_, _, _| {
                                crate::ui::notify::show(
                                    "fuwa".into(),
                                    "This is how messages will reach you ✨".into(),
                                    crate::ui::notify::Clicked {
                                        instance: String::new(),
                                        server: None,
                                        channel: String::new(),
                                    },
                                )
                            }),
                    ));
                ("Notifications".into(), "What fuwa tells you about.".into(), body.into_any_element())
            }
            Page::Streamer => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(18.0))
                    .child(toggle_row(
                        "streamer",
                        "Streamer mode",
                        "Hides instance addresses, your username, invite links and what private messages say in notifications. For streaming or sharing your screen.",
                        prefs.streamer_mode,
                        p,
                        cx,
                        |this, on, cx| this.set(cx, |pr| pr.streamer_mode = on),
                    ));
                ("Streamer mode".into(), "Keep private things off your stream.".into(), body.into_any_element())
            }
            Page::Accounts => {
                let streamer = prefs.streamer_mode;
                let accounts = self.core.shared.read(|s| {
                    s.order
                        .iter()
                        .filter_map(|k| s.instance(k))
                        .map(|i| (i.key.clone(), i.name(), i.url.clone(), i.me.clone(), i.connection))
                        .collect::<Vec<_>>()
                });
                let mut list = div().flex().flex_col().gap(px(12.0));
                for (n, (key, name, url, me, connection)) in accounts.into_iter().enumerate() {
                    let signed_out = connection == Connection::SignedOut;
                    let who = me
                        .as_ref()
                        .filter(|_| !signed_out)
                        .map(|m| if streamer { user_name(m) } else { format!("{} · @{}", user_name(m), m.username) });
                    let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
                    list = list.child(motion::rise(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .p(px(14.0))
                            .rounded(corner(16.0))
                            .bg(p.card)
                            .border_1()
                            .border_color(p.border)
                            .child(
                                div().relative().child(avatar(me.as_ref(), 44.0, p)).child(
                                    div()
                                        .absolute()
                                        .right(px(-2.0))
                                        .bottom(px(-2.0))
                                        .child(conn_dot(connection, p).border_color(p.card)),
                                ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(name))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(p.muted_foreground)
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(who.unwrap_or_else(|| "Signed out".into())),
                                    )
                                    .when(!streamer, |el| {
                                        el.child(div().text_xs().text_color(p.muted_foreground).child(url))
                                    }),
                            )
                            .child(if signed_out {
                                primary_button(SharedString::from(format!("again-{key}")), "Sign in", p)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(SettingsEvent::SignIn { key: k1.clone() })
                                    }))
                                    .into_any_element()
                            } else {
                                soft_button(SharedString::from(format!("out-{key}")), "Sign out", p)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let core = this.core.clone();
                                        let key = k2.clone();
                                        drop(core.spawn({
                                            let core = core.clone();
                                            async move { core.sign_out(&key).await }
                                        }));
                                        cx.notify();
                                    }))
                                    .into_any_element()
                            })
                            .child(icon_button(SharedString::from(format!("remove-{key}")), "trash", p).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.core.remove_instance(&k3);
                                    cx.notify();
                                }),
                            )),
                        SharedString::from(format!("acct-{key}")),
                        Duration::from_millis(40 * n as u64),
                        8.0,
                    ));
                }
                let body = div().flex().flex_col().gap(px(18.0)).child(list).child(
                    div().child(
                        soft_button("add-instance", "Add an instance", p)
                            .child(icon("plus").size(px(16.0)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::AddInstance))),
                    ),
                );
                (
                    "Accounts".into(),
                    "Every instance you're signed in to here. Signing out also wipes the private messages this computer kept for it."
                        .into(),
                    body.into_any_element(),
                )
            }
            Page::Keyboard => (
                "Keyboard".into(),
                "Shortcuts that work everywhere in the app. Click one to change it.".into(),
                self.keyboard_page(prefs, p, cx),
            ),
            Page::Privacy => (
                "Privacy".into(),
                "What this app tells anyone, and only if you let it.".into(),
                self.privacy_page(prefs, p, window, cx),
            ),
            Page::Updates => (
                "Updates".into(),
                "New versions of the app, checked before they run.".into(),
                self.updates_page(prefs, p, window, cx),
            ),
            Page::About => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .child(crate::ui::widgets::fuwa_mark(56.0, p))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_xl().font_weight(FontWeight::EXTRA_BOLD).child("fuwa desktop"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(p.muted_foreground)
                                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                                    ),
                            ),
                    )
                    .child(div().text_sm().text_color(p.muted_foreground).child(
                        "Made by Waifu Devs. Private messages are end-to-end encrypted with MLS; the font is M PLUS Rounded 1c (SIL Open Font License); icons are Lucide.",
                    ))
                    .child(div().flex().gap(px(10.0)).child(
                        soft_button("source", "Source code", p)
                            .on_click(|_, _, cx| cx.open_url("https://github.com/waifu-devs/fuwa")),
                    ).child(
                        soft_button("site", "waifu.dev", p).on_click(|_, _, cx| cx.open_url("https://www.waifu.dev/projects")),
                    ));
                ("About".into(), String::new(), body.into_any_element())
            }
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let prefs = self.core.prefs();
        sync_sliders(&self.look, &prefs.backdrop, _window, cx);
        let behind = crate::ui::backdrop::layers(&crate::ui::theme::backdrop(cx), &p, _window, cx);
        let mut menu = div().flex().flex_col().w(px(220.0));
        let mut y = 0.0;
        let mut at_y = 0.0;
        for (group, pages) in [("YOUR ACCOUNT", &ACCOUNT_PAGES[..]), ("APP SETTINGS", &PAGES[..])] {
            menu = menu.child(
                div()
                    .h(px(30.0))
                    .px(px(10.0))
                    .when(y > 0.0, |el| el.mt(px(14.0)))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(p.muted_foreground)
                    .child(group),
            );
            y += if y > 0.0 { 44.0 } else { 30.0 };
            for (page, glyph, label) in pages.iter().copied() {
                let on = page == self.page;
                if on {
                    at_y = y;
                }
                let hover = alpha(p.primary, 0.08);
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("menu-{label}")))
                        .h(px(38.0))
                        .mb(px(2.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .rounded(corner(10.0))
                        .cursor_pointer()
                        .text_color(if on { p.foreground } else { p.muted_foreground })
                        .when(on, |el| el.font_weight(FontWeight::BOLD))
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.page = page;
                            cx.notify();
                        }))
                        .child(icon(glyph).size(px(17.0)).text_color(if on { p.primary } else { p.muted_foreground }))
                        .child(label),
                );
                y += 40.0;
            }
        }
        let at = motion::follow("settings-hl", at_y, _window, cx);
        let menu = div()
            .relative()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(at))
                    .h(px(38.0))
                    .rounded(corner(10.0))
                    .bg(alpha(p.primary, 0.16)),
            )
            .child(menu);

        let (title, sub, body) = self.page(&prefs, &p, _window, cx);
        let page_key = ACCOUNT_PAGES
            .iter()
            .chain(PAGES.iter())
            .find(|(pg, _, _)| *pg == self.page)
            .map(|(_, _, l)| *l)
            .unwrap_or("x");
        let content = div()
            .w(px(640.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().text_2xl().font_weight(FontWeight::EXTRA_BOLD).child(title))
            .when(!sub.is_empty(), |el| el.child(div().text_color(p.muted_foreground).child(sub)))
            .child(div().h(px(18.0)))
            .child(body);

        motion::fade_in(
            div()
                .id("settings")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .bg(p.background)
                .when_some(behind, |el, behind| el.child(behind))
                .child(
                    div()
                        .flex_none()
                        .w(px(300.0))
                        .h_full()
                        .flex()
                        .justify_end()
                        .pt(px(56.0))
                        .pr(px(16.0))
                        .bg(p.side_surface)
                        .child(motion::slide_in(menu, "settings-menu", -24.0)),
                )
                .child(
                    div()
                        .id("settings-body")
                        .flex_1()
                        .h_full()
                        .bg(p.chat_surface)
                        .overflow_y_scroll()
                        .pt(px(56.0))
                        .px(px(40.0))
                        .pb(px(40.0))
                        .child(motion::rise(
                            content,
                            SharedString::from(format!("page-{page_key}")),
                            Duration::ZERO,
                            14.0,
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(20.0))
                        .right(px(24.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id("settings-close")
                                .size(px(38.0))
                                .rounded_full()
                                .border_2()
                                .border_color(p.muted_foreground)
                                .text_color(p.muted_foreground)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover({
                                    let c = p.primary;
                                    move |s| s.border_color(c).text_color(c)
                                })
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
                                .child(icon("x").size(px(18.0))),
                        )
                        .child(
                            div().text_xs().font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"),
                        ),
                ),
            "settings-in",
            Duration::from_millis(160),
        )
    }
}

pub(crate) fn section(title: &str, body: impl IntoElement, p: &Palette) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(body)
}

pub(crate) fn radio(on: bool, p: &Palette) -> impl IntoElement {
    div()
        .size(px(16.0))
        .rounded_full()
        .border_2()
        .border_color(if on { p.primary } else { p.muted_foreground })
        .flex()
        .items_center()
        .justify_center()
        .when(on, |el| el.child(div().size(px(6.0)).rounded_full().bg(p.primary)))
}

/// A tiny picture of the app in a palette.
pub(crate) fn theme_preview(pv: &Palette) -> impl IntoElement {
    div()
        .h(px(84.0))
        .flex()
        .rounded(px(10.0 * pv.radius))
        .overflow_hidden()
        .bg(pv.background)
        .child(div().w(px(16.0)).h_full().bg(pv.rail))
        .child(div().w(px(36.0)).h_full().bg(pv.sidebar))
        .child(
            div()
                .flex_1()
                .p(px(8.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(div().h(px(6.0)).w(px(40.0)).rounded_full().bg(pv.primary))
                .child(div().h(px(5.0)).w(px(56.0)).rounded_full().bg(alpha(pv.foreground, 0.4)))
                .child(div().h(px(5.0)).w(px(30.0)).rounded_full().bg(alpha(pv.foreground, 0.25)))
                .child(div().flex_1())
                .child(div().h(px(12.0)).rounded(px(4.0)).bg(pv.card).border_1().border_color(pv.border)),
        )
}

/// Choices side by side; the chosen one sits on a pill that glides between them.
pub(crate) fn segmented(
    id: &'static str,
    options: Vec<(&'static str, bool)>,
    p: &Palette,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    pick: impl Fn(&mut SettingsView, usize, &mut Context<SettingsView>) + Clone + 'static,
) -> impl IntoElement {
    let width = 150.0;
    let chosen = options.iter().position(|(_, on)| *on).unwrap_or(0);
    let mut row = div()
        .relative()
        .flex()
        .p(px(4.0))
        .rounded(corner(14.0))
        .bg(p.secondary)
        .w(px(width * options.len() as f32 + 8.0));
    // The pill moves with a spring, keyed to this control.
    let pill = gpui_kit::base::motion::spring(
        SharedString::from(format!("seg-{id}")),
        chosen as f32 * width,
        gpui_kit::base::motion::Spring::new(Duration::from_millis(340)).with_damping(0.75),
        window,
        cx,
    );
    row = row.child(
        div()
            .absolute()
            .top(px(4.0))
            .left(px(4.0 + pill))
            .w(px(width))
            .h(px(36.0))
            .rounded(corner(10.0))
            .bg(p.card)
            .shadow(vec![gpui_kit::BoxShadow {
                color: alpha(p.primary, 0.18),
                offset: gpui_kit::point(px(0.0), px(4.0)),
                blur_radius: px(12.0),
                spread_radius: px(-4.0),
                inset: false,
            }]),
    );
    for (n, (label, on)) in options.into_iter().enumerate() {
        let pick = pick.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("{id}-{n}")))
                .relative()
                .w(px(width))
                .h(px(36.0))
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .text_color(if on { p.primary } else { p.muted_foreground })
                .on_click(cx.listener(move |this, _, _, cx| pick(this, n, cx)))
                .child(label),
        );
    }
    row
}

pub(crate) fn toggle_row(
    id: &'static str,
    title: &str,
    body: &str,
    on: bool,
    p: &Palette,
    cx: &mut Context<SettingsView>,
    set: impl Fn(&mut SettingsView, bool, &mut Context<SettingsView>) + 'static,
) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let set = std::rc::Rc::new(set);
    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .p(px(16.0))
        .rounded(corner(16.0))
        .bg(p.card)
        .border_1()
        .border_color(p.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().font_weight(FontWeight::BOLD).child(title.to_owned()))
                .child(div().text_sm().text_color(p.muted_foreground).child(body.to_owned())),
        )
        .child(div().flex_none().child(Switch::new(id).checked(on).on_change(move |checked, _, cx| {
            let (set, checked) = (set.clone(), *checked);
            let _ = entity.update(cx, |this, cx| set(this, checked, cx));
        })))
}

/// A ball that bounces across, to show what the motion setting does.
fn bouncer(p: &Palette, reduced: bool) -> impl IntoElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let ball = div().size(px(22.0)).rounded_full().bg(p.primary);
    div().w(px(120.0)).h(px(40.0)).relative().child(if reduced {
        div().absolute().left(px(49.0)).top(px(9.0)).child(ball).into_any_element()
    } else {
        div()
            .absolute()
            .top(px(9.0))
            .child(ball)
            .with_animation("bouncer", Animation::new(Duration::from_millis(1600)).repeat(), |el, t| {
                let x = (t * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                let hop = ((t * 2.0 * std::f32::consts::TAU).sin()).abs();
                el.left(px(x * 98.0)).mt(px(-10.0 * hop))
            })
            .into_any_element()
    })
}
