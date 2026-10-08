//! The screen around instance settings, as the web's `SettingsScreen` draws
//! it for `InstanceSettingsDialog.tsx`: a side menu (the instance's name, a
//! search that finds pages and single settings, the Instance and Manage
//! groups) and the chosen page under its heading, with the close button and
//! the save bar that holds the screen while there are unsaved changes.

use std::time::{Duration, Instant};

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Window, div, point, px,
};

use super::{InstanceSettingsEvent, InstanceSettingsView, Page};
use crate::core::i18n::{Arg, t, t_with};
use crate::ui::motion;
use crate::ui::text::{WIDE, tracked};
use crate::ui::theme::{Palette, alpha, radius_lg, radius_md};
use crate::ui::widgets::icon;

/// How long the screen takes to come and go.
pub(super) const OPENING: Duration = Duration::from_millis(280);

/// A page of the menu, as the web's `SettingsSection`.
pub(super) struct Section {
    pub page: Page,
    pub glyph: &'static str,
    pub label: String,
    pub about: String,
    /// More words search finds it by.
    pub keywords: &'static str,
    /// Single settings search can jump to: (id, label, keywords).
    pub settings: Vec<(&'static str, String, &'static str)>,
}

impl Section {
    fn new(page: Page, glyph: &'static str, label: String, about: String, keywords: &'static str) -> Self {
        Self { page, glyph, label, about, keywords, settings: Vec::new() }
    }

    fn with(mut self, settings: Vec<(&'static str, String, &'static str)>) -> Self {
        self.settings = settings;
        self
    }
}

pub(super) struct Group {
    pub label: String,
    pub sections: Vec<Section>,
}

/// One setting search can find, in the app's language.
fn entry(id: &'static str, key: &str, keywords: &'static str) -> (&'static str, String, &'static str) {
    (id, t(key), keywords)
}

/// The menu: the instance's settings, then what an admin manages (the web's `settingsGroups`).
pub(super) fn groups(name: &str, items_here: bool) -> Vec<Group> {
    let instance = vec![
        Section::new(
            Page::General,
            "sliders-horizontal",
            t("instancesettings.nav.general"),
            t_with("instancesettings.nav.generalAbout", &[("name", Arg::Str(name))]),
            "",
        )
        .with(vec![
            entry("name", "instancesettings.nav.name", ""),
            entry("public-url", "instancesettings.nav.publicUrl", "url domain"),
            entry("web", "instancesettings.nav.web", ""),
            entry("origins", "instancesettings.nav.origins", "cors origins allowed"),
        ]),
        Section::new(
            Page::SignUps,
            "user-plus",
            t("instancesettings.nav.signUps"),
            t("instancesettings.nav.signUpsAbout"),
            "",
        )
        .with(vec![
            entry("local-accounts", "instancesettings.nav.localAccounts", "sign up password"),
            entry("linked-accounts", "instancesettings.nav.linkedAccounts", "linked sign in sign up"),
            entry("linked-issuer", "instancesettings.nav.linkedIssuer", "issuer openauth waifu.dev linked"),
            entry("server-creation", "instancesettings.nav.serverCreation", ""),
            entry("servers-per-account", "instancesettings.nav.serversPerAccount", ""),
            entry("agent-creation", "instancesettings.nav.agentCreation", "bots integrations"),
            entry("mcp", "instancesettings.nav.mcp", "mcp claude ai model context protocol"),
            entry("shared-channels", "serversettings.nav.shared", "share connect servers slack connect"),
            entry(
                "profile-effects",
                "instancesettings.nav.profileEffects",
                "sparkles petals animation card decoration",
            ),
            entry(
                "rich-presence",
                "instancesettings.nav.richPresence",
                "activity playing game status discord presence",
            ),
        ]),
        Section::new(
            Page::Sso,
            "building",
            t("serversettings.nav.sso"),
            t("instancesettings.nav.ssoAbout"),
            "sso saml oidc openid okta entra azure google workspace keycloak authentik identity provider",
        )
        .with(vec![
            entry("sso-accounts", "instancesettings.nav.ssoAccounts", "sso sign up"),
            entry("sso-protocol", "serversettings.nav.ssoProtocol", "saml oidc openid"),
            entry("sso-domains", "serversettings.nav.ssoDomains", "sso allowed"),
            entry("sso-test", "instancesettings.nav.ssoTest", "sso check"),
        ]),
        Section::new(
            Page::Providers,
            "log-in",
            t("instancesettings.nav.signInProviders"),
            t("instancesettings.nav.signInProvidersAbout"),
            "google x twitter twitch oauth social login sign in",
        )
        .with(vec![
            entry("provider-accounts", "instancesettings.nav.providerAccounts", "sign up new accounts"),
            ("provider-google", "Google".to_owned(), "client id secret app"),
            ("provider-x", "X".to_owned(), "client id secret app"),
            ("provider-twitch", "Twitch".to_owned(), "client id secret app"),
        ]),
        Section::new(Page::Limits, "gauge", t("serversettings.nav.limits"), t("instancesettings.nav.limitsAbout"), "")
            .with(vec![
                entry("default-limits", "instancesettings.nav.defaultLimits", "members channels storage attachments"),
                entry("picture-uploads", "instancesettings.nav.pictureUploads", "avatar banner icon image size"),
            ]),
        Section::new(
            Page::Privacy,
            "shield-check",
            t("instancesettings.nav.privacy"),
            t("instancesettings.nav.privacyAbout"),
            "",
        )
        .with(vec![entry(
            "telemetry",
            "instancesettings.nav.telemetry",
            "telemetry analytics errors performance",
        )]),
        Section::new(
            Page::Calls,
            "audio-lines",
            t("instancesettings.nav.calls"),
            t("instancesettings.nav.callsAbout"),
            "voice webrtc stun turn ice media",
        )
        .with(vec![
            entry("calls-on", "instancesettings.nav.calls", "voice enable"),
            entry("call-recordings", "instancesettings.nav.callRecordings", "record recordings tracks podcast"),
            entry(
                "call-recordings-keep",
                "instancesettings.nav.callRecordingsKeep",
                "retention expire delete days old recordings",
            ),
            entry("ice-urls", "instancesettings.nav.iceUrls", "ice nat relay firewall"),
            entry("turn-secret", "instancesettings.nav.turnSecret", "coturn relay password"),
        ]),
        Section::new(
            Page::Moderation,
            "shield-alert",
            t("instancesettings.nav.moderation"),
            t("instancesettings.nav.moderationAbout"),
            "automod ai jev typesafe cloudflare clef workers smart filter custom webhook own",
        )
        .with(vec![
            ("automod-typesafe-jev", "TypeSafe Jev".to_owned(), "automod ai moderation key"),
            ("automod-cloudflare-clef", "Cloudflare Clef".to_owned(), "automod ai moderation workers token account"),
            entry(
                "automod-custom",
                "instancesettings.nav.automodCustom",
                "automod custom webhook classifier own endpoint",
            ),
            entry(
                "automod-checks-per-day",
                "instancesettings.nav.automodChecks",
                "automod limit cap budget cost quota daily",
            ),
        ]),
        Section::new(
            Page::Federation,
            "network",
            t("instancesettings.nav.federation"),
            t("instancesettings.nav.federationAbout"),
            "federation federate instances share channels across key fingerprint block",
        )
        .with(vec![
            entry("federation", "instancesettings.nav.federationOn", "federation on off"),
            entry(
                "federation-identity",
                "instancesettings.nav.federationIdentity",
                "fingerprint key address rotate replace",
            ),
            entry("federation-check", "instancesettings.nav.federationCheck", "test reach ping"),
            entry("federation-peers", "instancesettings.nav.federationPeers", "pinned peers"),
            entry("federation-blocked", "instancesettings.nav.federationBlocked", "block list deny"),
            entry("federation-sends", "instancesettings.nav.federationSends", "limit cap rate flood shared remote"),
            entry("federation-people", "instancesettings.nav.federationPeople", "limit cap shared remote guests"),
            entry(
                "federation-files",
                "instancesettings.nav.federationFiles",
                "limit cap shared remote attachments bytes",
            ),
            entry(
                "federation-fetches",
                "instancesettings.nav.federationFetches",
                "limit cap shared remote attachments busy",
            ),
        ]),
        Section::new(
            Page::Gifs,
            "film",
            t("instancesettings.nav.gifs"),
            t("instancesettings.nav.gifsAbout"),
            "gif giphy klipy tenor search animated",
        )
        .with(vec![
            entry("gif-provider", "instancesettings.nav.gifProvider", "giphy klipy"),
            entry("gif-key", "instancesettings.nav.gifKey", "api key secret"),
            entry("gif-rating", "instancesettings.nav.gifRating", "nsfw safe content filter"),
            entry("gif-caps", "instancesettings.nav.gifCaps", "size limit rate searches per day"),
        ]),
    ];
    let mut manage = vec![
        Section::new(
            Page::Accounts,
            "users",
            t("instancesettings.nav.accounts"),
            t("instancesettings.nav.accountsAbout"),
            "users people disable ban reset password admin",
        ),
        Section::new(
            Page::Servers,
            "server",
            t("instancesettings.nav.servers"),
            t("instancesettings.nav.serversAbout"),
            "communities export backup delete caps usage storage",
        ),
        Section::new(
            Page::Announcement,
            "megaphone",
            t("instancesettings.nav.announcement"),
            t("instancesettings.nav.announcementAbout"),
            "banner maintenance notice news",
        )
        .with(vec![
            entry("announcement-text", "instancesettings.nav.announcementText", ""),
            entry("announcement-tone", "instancesettings.nav.announcementTone", "urgent warning info"),
            entry("announcement-ends", "instancesettings.nav.announcementEnds", "expire end"),
        ]),
    ];
    // Profile items, on instances that have them, sit between Servers and Announcement.
    if items_here {
        manage.insert(
            2,
            Section::new(
                Page::ProfileItems,
                "sparkles",
                t("serversettings.nav.profileItems"),
                t("instancesettings.nav.profileItemsAbout"),
                "profile effect effects decoration decorations avatar frame card sparkles",
            ),
        );
    }
    vec![
        Group { label: t("instancesettings.nav.instance"), sections: instance },
        Group { label: t("instancesettings.nav.manage"), sections: manage },
    ]
}

type Found<'a> = (&'a Section, Vec<&'a (&'static str, String, &'static str)>);

/// Pages and single settings whose words hold every word typed (the web's `search`).
fn search<'a>(groups: &'a [Group], query: &str) -> Option<Vec<Found<'a>>> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return None;
    }
    let hits = |hay: &str| {
        let hay = hay.to_lowercase();
        words.iter().all(|w| hay.contains(w.as_str()))
    };
    let mut out = Vec::new();
    for section in groups.iter().flat_map(|g| g.sections.iter()) {
        let own = format!("{} {} {}", section.label, section.about, section.keywords);
        let settings: Vec<_> =
            section.settings.iter().filter(|s| hits(&format!("{} {} {}", s.1, s.2, section.label))).collect();
        if !settings.is_empty() || hits(&own) {
            out.push((section, settings));
        }
    }
    Some(out)
}

/// A menu row sliding in from the left, a beat after the one above it.
fn slide(el: impl IntoElement + Styled + 'static, id: SharedString, n: usize) -> AnyElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let delay = 0.05 + n as f32 * 0.03;
    let total = Duration::from_secs_f32(delay + 0.4);
    let start = delay / total.as_secs_f32();
    el.with_animation(id, Animation::new(total), move |el, t| {
        let k = if t <= start { 0.0 } else { ((t - start) / (1.0 - start)).clamp(0.0, 1.0) };
        let eased = 1.0 - (1.0 - k).powi(3);
        el.opacity(eased).relative().left(px(-10.0 * (1.0 - eased)))
    })
    .into_any_element()
}

use gpui_kit::Styled;

impl InstanceSettingsView {
    /// The instance's name as the screen shows it.
    pub(super) fn name(&self) -> String {
        self.core.shared.read(|s| s.instance(&self.key).map(|i| i.name()).unwrap_or_else(|| self.key.clone()))
    }

    /// Opens a page (and, from search, one setting on it).
    pub(super) fn choose(&mut self, page: Page, setting: Option<&'static str>, cx: &mut Context<Self>) {
        if page != self.page {
            self.scroll.set_offset(point(px(0.0), px(0.0)));
            self.places.borrow_mut().clear();
        }
        self.open(page, cx);
        if let Some(id) = setting {
            let n = self.glow.map_or(0, |(_, n)| n + 1);
            self.glow = Some((id, n));
            self.scroll_to = Some(id);
        }
        cx.notify();
    }

    /// Enter in the search: the first thing it found.
    pub(super) fn pick_first(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let groups = groups(&self.name(), self.instance_has("profile-items"));
        let first = search(&groups, &query)
            .and_then(|results| results.first().map(|(s, settings)| (s.page, settings.first().map(|x| x.0))));
        if let Some((page, setting)) = first {
            self.choose(page, setting, cx);
        }
    }

    /// Someone tried to leave with unsaved changes: the save bar shakes.
    pub(super) fn hold_on(&mut self, cx: &mut Context<Self>) {
        self.nudge = (self.nudge.0 + 1, Some(Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1850)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The save bar's alarm, while someone just tried to leave.
    pub(super) fn alarm(&self) -> Option<u32> {
        match self.nudge {
            (n, Some(at)) if at.elapsed() < Duration::from_millis(1800) => Some(n),
            _ => None,
        }
    }

    /// Closes the screen, unless unsaved changes hold it.
    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        if !self.changed().is_empty() {
            self.hold_on(cx);
            return;
        }
        if self.closing.is_some() {
            return;
        }
        if cx.reduce_motion() {
            cx.emit(InstanceSettingsEvent::Close);
            return;
        }
        self.closing = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OPENING).await;
            let _ = this.update(cx, |_, cx| cx.emit(InstanceSettingsEvent::Close));
        })
        .detach();
    }

    /// The side menu: the instance, the search, then the groups (or what the search found).
    pub(super) fn menu(
        &mut self,
        groups: &[Group],
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.query.read(cx).value().to_string();
        let focused = gpui_kit::Focusable::focus_handle(self.query.read(cx), cx).is_focused(window);
        let search_box = div()
            .relative()
            .h(px(36.0))
            .rounded(radius_lg())
            .border_1()
            .border_color(if focused { alpha(p.primary, 0.5) } else { alpha(p.border, 0.0) })
            .bg(if focused { p.background.into() } else { alpha(p.muted, 0.7) })
            .when(focused, |el| {
                el.shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.1),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(4.0),
                    inset: false,
                }])
            })
            .flex()
            .items_center()
            .pl(px(32.0))
            .pr(px(32.0))
            .text_sm()
            .child(
                div()
                    .absolute()
                    .left(px(9.0))
                    .top(px(9.0))
                    .text_color(if focused { p.primary } else { p.muted_foreground })
                    .child(icon("search").size(px(16.0))),
            )
            .child(div().flex_1().min_w_0().child(Input::new(&self.query).appearance(false)))
            .when(!query.is_empty(), |el| {
                let (hover, fg) = (p.muted, p.foreground);
                el.child(
                    div()
                        .id("isettings-search-clear")
                        .absolute()
                        .right(px(5.0))
                        .top(px(5.0))
                        .size(px(24.0))
                        .rounded(radius_md())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.query.update(cx, |q, cx| q.set_value("", window, cx));
                            cx.notify();
                        }))
                        .child(icon("x").size(px(14.0))),
                )
            });

        let nav = div()
            .w(px(240.0))
            .flex_none()
            .pt(px(64.0))
            .pb(px(64.0))
            .pr(px(12.0))
            .pl(px(20.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .px(px(8.0))
                    .child(
                        div()
                            .text_lg()
                            .line_height(px(28.0))
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .truncate()
                            .child(self.name()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .truncate()
                            .child(t("instancesettings.nav.subtitle")),
                    ),
            )
            .child(search_box);

        if let Some(results) = search(groups, &query) {
            return nav.child(self.results(&results, &query, p, cx)).into_any_element();
        }

        // The highlight glides between rows: where the chosen one sits in the list.
        let mut start = 0.0;
        let mut at = None;
        let mut list = div().relative().flex().flex_col().gap(px(20.0));
        let mut blocks = Vec::new();
        let mut n = 0usize;
        for (g, group) in groups.iter().enumerate() {
            let mut block = div().flex().flex_col().gap(px(2.0));
            let mut y = start;
            if g > 0 {
                block = block.border_t_1().border_color(alpha(p.border, 0.7)).pt(px(12.0));
                y += 13.0;
            }
            block = block.child(
                div()
                    .mb(px(4.0))
                    .px(px(8.0))
                    .text_size(px(11.2))
                    .line_height(px(16.8))
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.muted_foreground)
                    .child(tracked(group.label.to_uppercase(), WIDE)),
            );
            y += 16.8 + 4.0 + 2.0;
            for (i, section) in group.sections.iter().enumerate() {
                if i > 0 {
                    y += 2.0;
                }
                let active = section.page == self.page;
                if active {
                    at = Some(y);
                }
                let page = section.page;
                let (hover_bg, hover_fg) = (alpha(p.muted, 0.7), p.foreground);
                let row = div()
                    .id(SharedString::from(format!("imenu-{page:?}")))
                    .relative()
                    .h(px(32.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .rounded(radius_lg())
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(if active { p.primary } else { p.muted_foreground })
                    .cursor_pointer()
                    .when(!active, |el| el.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.blur(cx);
                        this.choose(page, None, cx)
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(section.label.clone()));
                block = block.child(slide(row, SharedString::from(format!("imenu-in-{page:?}")), n));
                n += 1;
                y += 32.0;
            }
            start = y + 20.0;
            blocks.push(block);
        }
        if let Some(top) = at {
            let top = motion::follow("isettings-hl", top, window, cx);
            list = list.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top))
                    .h(px(32.0))
                    .rounded(radius_lg())
                    .bg(alpha(p.primary, 0.15)),
            );
        }
        list = list.children(blocks);
        nav.child(list).into_any_element()
    }

    /// What a search found: pages, and under each the single settings that matched.
    fn results(&mut self, results: &[Found<'_>], query: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if results.is_empty() {
            return motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(24.0))
                    .text_sm()
                    .text_color(p.muted_foreground)
                    .child(motion::once(
                        div().child(icon("search-x").size(px(28.0))),
                        SharedString::from(format!("inomatch-{query}")),
                        Duration::from_millis(700),
                        |el, t| {
                            let k = if t < 1.0 / 7.0 { 0.0 } else { (t - 1.0 / 7.0) * 7.0 / 6.0 };
                            let wiggle = (k * std::f32::consts::TAU * 2.0).sin() * (1.0 - k) * 2.0;
                            el.relative().left(px(wiggle))
                        },
                    ))
                    .child(t_with("settings.screen.noMatches", &[("query", Arg::Str(query))])),
                "isettings-nomatch",
                Duration::ZERO,
                8.0,
            )
            .into_any_element();
        }
        let mut list = div().flex().flex_col().gap(px(2.0));
        let mut n = 0;
        let hover = alpha(p.muted, 0.7);
        for (section, settings) in results {
            let page = section.page;
            list = list.child(slide(
                div()
                    .id(SharedString::from(format!("iresult-{page:?}")))
                    .h(px(32.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(radius_lg())
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(p.foreground)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.blur(cx);
                        this.choose(page, None, cx)
                    }))
                    .child(icon(section.glyph).size(px(16.0)))
                    .child(div().truncate().child(section.label.clone())),
                SharedString::from(format!("iresult-in-{page:?}")),
                n,
            ));
            n += 1;
            for (id, label, _) in settings.iter().copied() {
                let id: &'static str = id;
                let fg = p.foreground;
                list = list.child(slide(
                    div()
                        .id(SharedString::from(format!("iresult-{page:?}-{id}")))
                        .h(px(30.0))
                        .pl(px(24.0))
                        .pr(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(radius_lg())
                        .text_size(px(12.8))
                        .text_color(p.muted_foreground)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover).text_color(fg))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(page, Some(id), cx)))
                        .child(div().opacity(0.6).child(icon("corner-down-right").size(px(14.0))))
                        .child(div().truncate().child(label.clone())),
                    SharedString::from(format!("iresult-in-{page:?}-{id}")),
                    n,
                ));
                n += 1;
            }
        }
        list.into_any_element()
    }

    /// The close button beside the page, with ESC under it.
    pub(super) fn close_button(&self, left: f32, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let (hover_bg, hover_fg, hover_ring) = (p.muted, p.foreground, alpha(p.foreground, 0.4));
        div()
            .id("isettings-close")
            .absolute()
            .top(px(64.0))
            .left(px(left))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .id("isettings-close-ring")
                    .size(px(40.0))
                    .rounded_full()
                    .border_2()
                    .border_color(p.border)
                    .text_color(p.muted_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |s| s.bg(hover_bg).text_color(hover_fg).border_color(hover_ring))
                    .active(|s| s.top(px(1.0)))
                    .child(icon("x").size(px(20.0))),
            )
            .child(div().text_size(px(10.4)).font_weight(FontWeight::BOLD).text_color(p.muted_foreground).child("ESC"))
            .into_any_element()
    }
}
