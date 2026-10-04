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

use super::general::hidden;
use super::{HIDDEN_ADDRESS, InstanceSettingsView};
use crate::core::dms::now_ms;
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
                .child(div().flex_1().min_w_0().text_color(p.muted_foreground).child(
                    "With this on, this instance talks to other fuwa instances so servers can share channels across \
                     them. Only the instances talk: apps here never connect to another instance, and no one's address \
                     is passed on. Every call is signed with this instance's key and checked against the key pinned \
                     for the other one. Sharing channels across instances comes in a later update; for now you can \
                     check that two instances reach each other.",
                )),
            "federation-intro",
            Duration::ZERO,
            8.0,
        );

        let mut page = div().flex().flex_col().child(intro).child(self.setting(
            "federation",
            "Talk to other instances",
            None,
            &["federation"],
            if defaults.federation { "on" } else { "off" },
            0,
            self.toggle(
                "federation",
                draft.federation,
                false,
                "Share channels with other instances",
                "Off, this instance answers other instances with nothing but \u{201c}off\u{201d}. Turning it off later \
                 stops every call between instances.",
                p,
                cx,
                |d, on| d.federation = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "federation-identity",
            "This instance's key",
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
            "Check an instance",
            Some(
                "Fetches its key and pins it here, then sends it a signed greeting there and back. The other instance \
                 only checks the greeting: it pins this one's key when its own admins check this instance.",
            ),
            &[],
            "",
            2,
            check,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "federation-peers",
            "Instances this one knows",
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
            "Blocked instances",
            Some("Host names, one a line. This instance never calls them and turns their calls away."),
            &["federation_blocked_hosts"],
            "none",
            4,
            list,
            p,
            cx,
        ));
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
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.0))
                .text_sm()
                .child("Other instances know this one as")
                .child(div().font_family("monospace").text_xs().font_weight(FontWeight::BOLD).child(if hide {
                    HIDDEN_ADDRESS.to_owned()
                } else {
                    shown(&info.origin)
                }))
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
            .child(div().text_xs().text_color(p.muted_foreground).child(
                "Before sharing with another instance, compare fingerprints with its admins somewhere you trust: \
                 theirs shows here once the two have met.",
            ))
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
            .child("Check")
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
                el.child(
                    div()
                        .text_xs()
                        .text_color(p.muted_foreground)
                        .child("Turn on \u{201c}Share channels with other instances\u{201d} and save first."),
                )
            });
        if let Some(result) = &f.result {
            let (tint, glyph, text, key) = match result {
                Ok(r) => {
                    let peer = r.peer.clone().unwrap_or_default();
                    let host = if hide { HIDDEN_ADDRESS.to_owned() } else { shown(&peer.origin) };
                    let both = if r.known_there {
                        "It knows this instance too, so signed calls go both ways."
                    } else {
                        "It doesn't know this instance yet: its admins check this one from their side to pin its key."
                    };
                    (
                        green(),
                        "check",
                        format!(
                            "Reached {host} and back in {} ms. Its key: {}. {both}",
                            r.round_trip_ms, peer.fingerprint
                        ),
                        format!("ok-{}", peer.origin),
                    )
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
                .child("None yet. An instance shows here once an admin here checks it.")
                .into_any_element();
        }
        let now = now_ms();
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, peer) in peers.iter().take(f.shown).enumerate() {
            let heard = match &peer.last_heard {
                Some(t) => match active_ago(t.seconds * 1000, now).trim_start_matches("Active ") {
                    "now" => "just now".to_owned(),
                    ago => ago.to_owned(),
                },
                None => "never".to_owned(),
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
                                    .child(if hide { HIDDEN_ADDRESS.to_owned() } else { shown(&peer.origin) })
                                    .when(peer.blocked, |el| el.child(pill("BLOCKED", p.destructive.into()))),
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
                    .child(
                        div().flex_none().text_xs().text_color(p.muted_foreground).child(format!("Heard from {heard}")),
                    ),
                SharedString::from(format!("federation-peer-{}", peer.origin)),
                Duration::from_millis(30 * n.min(10) as u64),
                6.0,
            ));
        }
        if peers.len() > f.shown {
            let more = (peers.len() - f.shown).min(PAGE);
            list = list.child(div().child(soft_button("federation-more", format!("Show {more} more"), p).on_click(
                cx.listener(|this, _, _, cx| {
                    this.federation.shown += PAGE;
                    cx.notify();
                }),
            )));
        }
        list.into_any_element()
    }
}
