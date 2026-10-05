//! Other instances: the federation switch, this instance's address and key
//! fingerprint for other admins to compare, checking that another instance
//! can be reached, the instances this one has pinned a key for, and the
//! block list. The web's `settings/instance/Federation.tsx`.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, hsla,
    px,
};

use super::controls::{ago_text, count_text, per_minute_text, size_text};
use super::general::hidden;
use super::signups::on_off;
use super::{InstanceSettingsView, hidden_address};
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_manage::active_ago;
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::{amber, pill, spinner};
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{icon, primary_button, soft_button};

/// How many known instances show before "Show more".
const PAGE: usize = 50;

pub(super) struct Federation {
    info: Option<pb::GetFederationResponse>,
    error: Option<String>,
    /// The saved settings the info was read for; read again when they change.
    read_for: Option<String>,
    address: Entity<InputState>,
    checking: bool,
    result: Option<Result<pb::CheckInstanceResponse, String>>,
    shown: usize,
    /// The block list shown anyway in streamer mode.
    revealed: bool,
}

impl Federation {
    pub fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("chat.example.com"));
        let sub = cx.subscribe_in(
            &address,
            window,
            |this: &mut InstanceSettingsView, _, e: &InputEvent, window, cx| match e {
                InputEvent::PressEnter { .. } => this.check_instance(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        (
            Self {
                info: None,
                error: None,
                read_for: None,
                address,
                checking: false,
                result: None,
                shown: PAGE,
                revealed: false,
            },
            vec![sub],
        )
    }
}

/// An origin as people read it: the host, and the port when there is one.
fn shown(origin: &str) -> String {
    origin.split_once("://").map_or(origin, |(_, rest)| rest).to_owned()
}

/// An instance's message with its first letter up, as the web shows them.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn green() -> Hsla {
    hsla(0.42, 0.65, 0.45, 1.0)
}

fn sky(p: &Palette) -> Hsla {
    hsla(0.55, 0.85, if p.dark { 0.65 } else { 0.42 }, 1.0)
}

impl InstanceSettingsView {
    /// Reads who this instance is to others, once per saved switch, address and block list.
    fn read_federation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(saved) = self.saved() else { return };
        let stamp = format!("{}|{}|{}", saved.federation, saved.public_url, saved.federation_blocked_hosts.join(","));
        if self.federation.read_for.as_deref() == Some(stamp.as_str()) {
            return;
        }
        self.federation.read_for = Some(stamp.clone());
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.federation(&key).await }, move |this, result, _, cx| {
            if this.federation.read_for.as_deref() != Some(stamp.as_str()) {
                return;
            }
            match result {
                Ok(info) => {
                    this.federation.info = Some(info);
                    this.federation.error = None;
                }
                Err(problem) => this.federation.error = Some(problem.message),
            }
            cx.notify();
        });
    }

    fn check_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address = self.federation.address.read(cx).value().trim().to_owned();
        let on = self.saved().is_some_and(|s| s.federation);
        if address.is_empty() || self.federation.checking || !on {
            return;
        }
        self.federation.checking = true;
        self.federation.result = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.check_instance(&key, &address).await }, |this, result, _, cx| {
            let f = &mut this.federation;
            f.checking = false;
            match result {
                Ok(res) => {
                    // The just-checked instance goes first in the list.
                    if let (Some(info), Some(peer)) = (&mut f.info, &res.peer) {
                        info.peers.retain(|p| p.origin != peer.origin);
                        info.peers.insert(0, peer.clone());
                    }
                    f.result = Some(Ok(res));
                }
                Err(problem) => f.result = Some(Err(problem.message)),
            }
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn federation_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.read_federation(window, cx);
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let defaults = config.defaults.clone().unwrap_or_default();
        let saved_on = self.saved().is_some_and(|s| s.federation);
        let hide = self.core.prefs().streamer_mode;
        let blue = sky(p);

        let intro = motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .mb(px(16.0))
                .p(px(12.0))
                .rounded(corner(16.0))
                .border_1()
                .border_color(blue.opacity(0.3))
                .bg(blue.opacity(0.05))
                .text_sm()
                .child(
                    div()
                        .size(px(32.0))
                        .flex_none()
                        .rounded(corner(12.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(blue.opacity(0.15))
                        .child(motion::once(
                            icon("network").size(px(16.0)).text_color(blue),
                            "federation-intro-icon",
                            Duration::from_millis(1600),
                            // A little wave hello.
                            |el, t| el.rotate(gpui_kit::radians((t * std::f32::consts::TAU).sin() * (1.0 - t) * 0.14)),
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(p.muted_foreground)
                        .child(t("desktop.instance.federationIntro")),
                ),
            "federation-intro",
            Duration::ZERO,
            8.0,
        );

        let mut page = div().flex().flex_col().child(intro).child(self.setting(
            "federation",
            &t("instancesettings.nav.federationOn"),
            None,
            &["federation"],
            &on_off(defaults.federation),
            0,
            self.toggle(
                "federation",
                draft.federation,
                false,
                &t("instancesettings.federation.label"),
                &t("instancesettings.federation.hint"),
                p,
                cx,
                |d, on| d.federation = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "federation-identity",
            &t("instancesettings.nav.federationIdentity"),
            None,
            &[],
            "",
            1,
            self.federation_identity(hide, p),
            p,
            cx,
        ));
        let check = self.check_card(saved_on, hide, p, window, cx);
        page = page.child(self.setting(
            "federation-check",
            &t("instancesettings.nav.federationCheck"),
            Some(&t("instancesettings.federation.checkHint")),
            &[],
            "",
            2,
            check,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "federation-peers",
            &t("instancesettings.nav.federationPeers"),
            None,
            &[],
            "",
            3,
            self.peers(hide, p, cx),
            p,
            cx,
        ));
        let list: AnyElement = match self.areas.get("federation_blocked_hosts") {
            Some(_) if hide && !self.federation.revealed => div()
                .id("federation-blocked-reveal")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.federation.revealed = true;
                    cx.notify();
                }))
                .child(hidden(p))
                .into_any_element(),
            Some(state) => Textarea::new(state).into_any_element(),
            None => div().into_any_element(),
        };
        page = page.child(self.setting(
            "federation-blocked",
            &t("instancesettings.nav.federationBlocked"),
            Some(&t("instancesettings.federation.blockedHint")),
            &["federation_blocked_hosts"],
            &t("instancesettings.shared.none"),
            4,
            list,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "federation-sends",
            &t("instancesettings.nav.federationSends"),
            Some(&t("instancesettings.federation.sendsHint")),
            &["shared_remote_sends_per_minute"],
            &per_minute_text(defaults.shared_remote_sends_per_minute),
            5,
            self.cap("shared_remote_sends_per_minute", &t("instancesettings.shared.upTo"), false, p, window, cx),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "federation-people",
            &t("instancesettings.nav.federationPeople"),
            Some(&t("instancesettings.federation.peopleHint")),
            &["shared_remote_people"],
            &count_text(defaults.shared_remote_people),
            6,
            self.cap("shared_remote_people", &t("instancesettings.shared.upTo"), false, p, window, cx),
            p,
            cx,
        ));
        // Files in channels shared with other instances, where this instance sends them.
        if self.instance_has("shared-files-elsewhere") {
            page = page.child(self.setting(
                "federation-files",
                &t("instancesettings.nav.federationFiles"),
                Some(&t("instancesettings.federation.filesHint")),
                &["shared_remote_file_bytes_per_day"],
                &size_text(defaults.shared_remote_file_bytes_per_day),
                7,
                self.cap("shared_remote_file_bytes_per_day", &t("instancesettings.shared.upTo"), true, p, window, cx),
                p,
                cx,
            ));
            page = page.child(self.setting(
                "federation-fetches",
                &t("instancesettings.nav.federationFetches"),
                Some(&t("desktop.instance.fetchesHint")),
                &["shared_file_fetches_in_flight"],
                &count_text(defaults.shared_file_fetches_in_flight),
                8,
                self.cap("shared_file_fetches_in_flight", &t("instancesettings.shared.upTo"), false, p, window, cx),
                p,
                cx,
            ));
        }
        page.into_any_element()
    }

    /// The address other instances know this one by, and its key's fingerprint.
    fn federation_identity(&self, hide: bool, p: &Palette) -> AnyElement {
        let f = &self.federation;
        if let Some(error) = &f.error {
            return div().text_xs().text_color(p.muted_foreground).child(error.clone()).into_any_element();
        }
        let Some(info) = &f.info else {
            return div().h(px(64.0)).rounded(corner(12.0)).bg(alpha(p.muted_foreground, 0.1)).into_any_element();
        };
        let address: AnyElement = if info.origin.is_empty() {
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(corner(12.0))
                .bg(amber(p).opacity(0.1))
                .text_xs()
                .text_color(amber(p))
                .child(icon("triangle-alert").size(px(14.0)).mt(px(1.0)))
                .child(div().flex_1().min_w_0().child(capitalized(&info.origin_problem)))
                .into_any_element()
        } else {
            // The address sits in the sentence in its own type, wherever the language puts it.
            let line = t_with("desktop.instance.knownAs", &[("origin", Arg::Str("\u{E000}"))]);
            let (before, after) = line.split_once('\u{E000}').unwrap_or((line.as_str(), ""));
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.0))
                .text_sm()
                .when(!before.trim().is_empty(), |el| el.child(before.trim().to_owned()))
                .child(div().font_family("monospace").text_xs().font_weight(FontWeight::BOLD).child(if hide {
                    hidden_address()
                } else {
                    shown(&info.origin)
                }))
                .when(!after.trim().is_empty(), |el| el.child(after.trim().to_owned()))
                .into_any_element()
        };
        let mut groups = div().flex().flex_wrap().gap_x(px(12.0)).gap_y(px(4.0)).font_family("monospace").text_xs();
        for (n, group) in info.fingerprint.split(' ').enumerate() {
            groups = groups.child(motion::rise(
                div().child(group.to_owned()),
                SharedString::from(format!("federation-print-{n}-{group}")),
                Duration::from_millis(30 * n as u64),
                4.0,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(address)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(corner(12.0))
                    .bg(alpha(p.muted, 0.6))
                    .child(icon("fingerprint-pattern").size(px(20.0)).text_color(p.primary))
                    .child(div().flex_1().min_w_0().child(groups)),
            )
            .child(div().text_xs().text_color(p.muted_foreground).child(t("instancesettings.federation.compare")))
            .into_any_element()
    }

    fn check_card(
        &mut self,
        on: bool,
        hide: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let f = &self.federation;
        let typed = !f.address.read(cx).value().trim().is_empty();
        let ready = on && typed && !f.checking;
        let button = primary_button("federation-check-go", "", p)
            .when(!ready, |el| el.opacity(0.5))
            .child(if f.checking {
                spinner("federation-checking", 16.0, window)
            } else {
                icon("radar").size(px(16.0)).into_any_element()
            })
            .child(t("instancesettings.federation.check"))
            .on_click(cx.listener(|this, _, window, cx| this.check_instance(window, cx)));
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().min_w_0().when(!on, |el| el.opacity(0.6)).child(Input::new(&f.address)))
                    .child(button),
            )
            .when(!on, |el| {
                el.child(div().text_xs().text_color(p.muted_foreground).child(t_with(
                    "instancesettings.federation.turnOnFirst",
                    &[("setting", Arg::Str(&t("instancesettings.federation.label")))],
                )))
            });
        if let Some(result) = &f.result {
            let (tint, glyph, text, key) = match result {
                Ok(r) => {
                    let peer = r.peer.clone().unwrap_or_default();
                    let host = if hide { hidden_address() } else { shown(&peer.origin) };
                    let reached = t_with(
                        "instancesettings.federation.reached",
                        &[
                            ("origin", Arg::Str(&host)),
                            ("ms", Arg::Str(&r.round_trip_ms.to_string())),
                            ("fingerprint", Arg::Str(&peer.fingerprint)),
                        ],
                    );
                    let both = if r.known_there {
                        t("instancesettings.federation.knownThere")
                    } else {
                        t("instancesettings.federation.notKnownThere")
                    };
                    (green(), "check", format!("{reached} {both}"), format!("ok-{}", peer.origin))
                }
                Err(message) => (p.destructive.into(), "triangle-alert", message.clone(), format!("error-{message}")),
            };
            card = card.child(motion::rise(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(corner(12.0))
                    .bg(tint.opacity(0.1))
                    .text_xs()
                    .text_color(tint)
                    .child(icon(glyph).size(px(14.0)).mt(px(1.0)))
                    .child(div().flex_1().min_w_0().child(capitalized(&text))),
                SharedString::from(format!("federation-result-{key}")),
                Duration::ZERO,
                6.0,
            ));
        }
        card.into_any_element()
    }

    fn peers(&self, hide: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.federation;
        let peers = f.info.as_ref().map(|i| i.peers.as_slice()).unwrap_or_default();
        if peers.is_empty() {
            return div()
                .text_xs()
                .text_color(p.muted_foreground)
                .child(t("instancesettings.federation.noPeers"))
                .into_any_element();
        }
        let now = now_ms();
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, peer) in peers.iter().take(f.shown).enumerate() {
            let heard = match &peer.last_heard {
                Some(at) => {
                    let when = ago_text(active_ago(at.seconds * 1000, now)).unwrap_or_else(|| t("common.time.justNow"));
                    t_with("instancesettings.federation.heard", &[("when", Arg::Str(&when))])
                }
                None => t("instancesettings.federation.heardNever"),
            };
            let edge: Hsla = if peer.blocked { alpha(p.destructive, 0.4) } else { p.border.into() };
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(corner(12.0))
                    .border_1()
                    .border_color(edge)
                    .when(peer.blocked, |el| el.bg(alpha(p.destructive, 0.05)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child(if hide { hidden_address() } else { shown(&peer.origin) })
                                    .when(peer.blocked, |el| {
                                        el.child(pill(
                                            &t("instancesettings.federation.blocked").to_uppercase(),
                                            p.destructive.into(),
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_size(px(11.0))
                                    .text_color(p.muted_foreground)
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(peer.fingerprint.clone()),
                            ),
                    )
                    .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(heard)),
                SharedString::from(format!("federation-peer-{}", peer.origin)),
                Duration::from_millis(30 * n.min(10) as u64),
                6.0,
            ));
        }
        if peers.len() > f.shown {
            let more = (peers.len() - f.shown).min(PAGE);
            list = list.child(
                div().child(
                    soft_button(
                        "federation-more",
                        t_with("instancesettings.federation.showMore", &[("count", Arg::Num(more as i64))]),
                        p,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.federation.shown += PAGE;
                        cx.notify();
                    })),
                ),
            );
        }
        list.into_any_element()
    }
}
