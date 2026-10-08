//! Other instances: the federation switch, this instance's address and key
//! fingerprint for other admins to compare (and replacing the key), checking
//! that another instance can be reached, the instances this one has pinned a
//! key for, the block list and the caps on what other instances send. The
//! web's `settings/instance/Federation.tsx`.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use super::controls::{amber_text, area_box, dialog, dialog_buttons, input_box, shimmer};
use super::general::hidden;
use super::{HIDDEN_ADDRESS, InstanceSettingsEvent, InstanceSettingsView};
use crate::core::dms::now_ms;
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin as admin;
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::settings_controls::{Look, button};
use crate::ui::text::{ago, ms_of};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_xl};
use crate::ui::widgets::icon;

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
    /// "Rotate this instance's key?" is open, and whether it's on its way.
    rotating: bool,
    rotate_busy: bool,
    rotate_error: Option<String>,
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
                rotating: false,
                rotate_busy: false,
                rotate_error: None,
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

/// `sky-500`, the page's own color.
fn sky() -> gpui_kit::Rgba {
    gpui_kit::rgb(0x00a6f4)
}

/// `text-emerald-700 dark:text-emerald-300`.
fn emerald(p: &Palette) -> Hsla {
    gpui_kit::rgb(if p.dark { 0x5ee9b5 } else { 0x007a55 }).into()
}

/// A sentence from the catalog with its placeholders filled, some in bold or monospace.
fn sentence(key: &str, count: Option<i64>, parts: &[(&str, &str, bool)], p: &Palette) -> gpui_kit::StyledText {
    let mut args: Vec<(&str, Arg)> = Vec::new();
    if let Some(n) = count {
        args.push(("count", Arg::Num(n)));
    }
    // Each placeholder stays as written, so it can be found and styled below.
    let template = t_with(key, &args);
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut rest = template.as_str();
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        text.push_str(&rest[..open]);
        let name = &rest[open + 1..open + close];
        match parts.iter().find(|(n, _, _)| *n == name) {
            Some((_, value, mono)) => {
                let start = text.len();
                text.push_str(value);
                let style = if *mono {
                    gpui_kit::HighlightStyle { ..Default::default() }
                } else {
                    gpui_kit::HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        color: Some(p.foreground.into()),
                        ..Default::default()
                    }
                };
                runs.push((start..text.len(), style));
            }
            None => text.push_str(&rest[open..open + close + 1]),
        }
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    gpui_kit::StyledText::new(text).with_highlights(runs)
}

impl InstanceSettingsView {
    /// Reads who this instance is to others, once per saved switch, address and block list (and
    /// again after the key is replaced).
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

    /// Replaces the key, then reads the page again and says the new fingerprint's start.
    fn rotate_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.federation.rotate_busy {
            return;
        }
        self.federation.rotate_busy = true;
        self.federation.rotate_error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(window, cx, async move { core.rotate_federation_key(&key).await }, |this, result, window, cx| {
            this.federation.rotate_busy = false;
            match result {
                Ok(fingerprint) => {
                    this.federation.rotating = false;
                    this.federation.read_for = None;
                    this.read_federation(window, cx);
                    let start = fingerprint.split(' ').take(2).collect::<Vec<_>>().join(" ");
                    cx.emit(InstanceSettingsEvent::Toast {
                        icon: "key-round",
                        title: t_with("instancesettings.federation.newKey", &[("fingerprint", Arg::Str(&start))]),
                    });
                }
                Err(problem) => this.federation.rotate_error = Some(capitalized(&problem.message)),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Esc closes "Rotate this instance's key?" first.
    pub(super) fn close_federation_dialog(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.federation.rotating {
            return false;
        }
        if !self.federation.rotate_busy {
            self.federation.rotating = false;
            self.federation.rotate_error = None;
            cx.notify();
        }
        true
    }

    /// The web's `ConfirmDialog` for replacing the key.
    pub(super) fn federation_dialog(
        &mut self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let f = &self.federation;
        if !f.rotating {
            return None;
        }
        let busy = f.rotate_busy;
        let body = div()
            .flex()
            .flex_col()
            .when_some(f.rotate_error.clone(), |el, e| {
                el.child(div().mb(px(12.0)).text_sm().text_color(p.destructive).child(e))
            })
            .child(
                dialog_buttons()
                    .child(
                        button("rotate-keep", t("serversettings.shared.keepIt"), None, Look::Ghost, false, p)
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_federation_dialog(cx);
                            })),
                    )
                    .child(
                        button(
                            "rotate-it",
                            t("instancesettings.federation.rotateIt"),
                            None,
                            Look::Destructive,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .when(busy, |el| el.opacity(0.5).child(spinner("rotate-spin", 16.0, window)))
                        .on_click(cx.listener(|this, _, window, cx| this.rotate_key(window, cx))),
                    ),
            );
        Some(dialog(
            "rotate-dialog",
            t("instancesettings.federation.rotateAsk"),
            Some(t("instancesettings.federation.rotateBody")),
            body,
            p,
            cx,
            |this, cx| {
                this.close_federation_dialog(cx);
            },
        ))
    }

    pub(super) fn federation_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.read_federation(window, cx);
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let defaults = config.defaults.clone().unwrap_or_default();
        let saved_on = self.saved().is_some_and(|s| s.federation);
        let hide = self.core.prefs().streamer_mode;
        let blue = sky();

        // `mb-4 flex items-start gap-3 rounded-2xl border border-sky-500/30 bg-sky-500/5 p-3 text-sm`.
        let intro = motion::rise(
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .mb(px(16.0))
                .p(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(alpha(blue, 0.3))
                .bg(alpha(blue, 0.05))
                .text_sm()
                .line_height(px(20.0))
                .child(
                    div()
                        .size(px(32.0))
                        .flex_none()
                        .rounded(radius_xl())
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(alpha(blue, 0.15))
                        .text_color(gpui_kit::rgb(if p.dark { 0x74d4ff } else { 0x0084d1 }))
                        .child(motion::ambient(
                            icon("network").size(px(16.0)),
                            "federation-intro-icon",
                            Duration::from_millis(7600),
                            window,
                            // A little wave every few seconds (1.6s, then 6s still).
                            |el, t| {
                                let k = (t * 7600.0 / 1600.0).min(1.0);
                                el.rotate(gpui_kit::radians((k * std::f32::consts::TAU).sin() * (1.0 - k) * 0.14))
                            },
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.federation.intro")),
                ),
            "federation-intro",
            Duration::ZERO,
            8.0,
        );

        let on_off = |on: bool| t(if on { "instancesettings.shared.on" } else { "instancesettings.shared.off" });
        let mut page = div().flex().flex_col().child(intro).child(self.setting_ruled(
            "federation",
            &t("instancesettings.nav.federationOn"),
            None,
            &["federation"],
            &on_off(defaults.federation),
            1,
            false,
            true,
            self.toggle(
                "federation",
                draft.federation,
                false,
                &t("instancesettings.federation.label"),
                &t("instancesettings.federation.hint"),
                p,
                window,
                cx,
                |d, on| d.federation = on,
            ),
            p,
            cx,
        ));
        let identity = self.federation_identity(hide, p, cx);
        page = page.child(self.setting(
            "federation-identity",
            &t("instancesettings.nav.federationIdentity"),
            None,
            &[],
            "",
            1,
            identity,
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
            Some(state) => {
                focus_ring(area_box(Textarea::new(state).appearance(false), None, p), has_focus(state, window, cx), p)
                    .font_family("monospace")
                    .text_xs()
                    .into_any_element()
            }
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
        let up_to = t("instancesettings.shared.upTo");
        let sends_default = defaults.shared_remote_sends_per_minute.map_or_else(
            || t("instancesettings.shared.noLimit"),
            |n| t_with("instancesettings.shared.perMinute", &[("count", Arg::Num(n))]),
        );
        let sends = self.cap_with("shared_remote_sends_per_minute", &up_to, false, Some("120"), p, window, cx);
        page = page.child(self.setting(
            "federation-sends",
            &t("instancesettings.nav.federationSends"),
            Some(&t("instancesettings.federation.sendsHint")),
            &["shared_remote_sends_per_minute"],
            &sends_default,
            5,
            sends,
            p,
            cx,
        ));
        let people = self.cap_with("shared_remote_people", &up_to, false, Some("500"), p, window, cx);
        page = page.child(self.setting(
            "federation-people",
            &t("instancesettings.nav.federationPeople"),
            Some(&t("instancesettings.federation.peopleHint")),
            &["shared_remote_people"],
            &admin::count_label(defaults.shared_remote_people),
            6,
            people,
            p,
            cx,
        ));
        // Files in channels shared with other instances, where this instance sends them.
        if self.instance_has("shared-files-elsewhere") {
            let files = self.cap("shared_remote_file_bytes_per_day", &up_to, true, p, window, cx);
            page = page.child(self.setting(
                "federation-files",
                &t("instancesettings.nav.federationFiles"),
                Some(&t("instancesettings.federation.filesHint")),
                &["shared_remote_file_bytes_per_day"],
                &admin::size_label(defaults.shared_remote_file_bytes_per_day),
                7,
                files,
                p,
                cx,
            ));
            let fetches = self.cap_with("shared_file_fetches_in_flight", &up_to, false, Some("8"), p, window, cx);
            page = page.child(self.setting(
                "federation-fetches",
                &t("instancesettings.nav.federationFetches"),
                Some(&t("instancesettings.federation.fetchesHint")),
                &["shared_file_fetches_in_flight"],
                &admin::count_label(defaults.shared_file_fetches_in_flight),
                8,
                fetches,
                p,
                cx,
            ));
        }
        page.into_any_element()
    }

    /// The address other instances know this one by, its key's fingerprint, and replacing the key.
    fn federation_identity(&self, hide: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let f = &self.federation;
        if let Some(error) = &f.error {
            return div().text_xs().text_color(p.muted_foreground).child(capitalized(error)).into_any_element();
        }
        let Some(info) = &f.info else {
            return shimmer(0, 64.0, p);
        };
        let address: AnyElement = if info.origin.is_empty() {
            // `flex items-start gap-2 rounded-xl bg-amber-500/10 px-3 py-2 text-xs`.
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(8.0))
                .rounded(radius_xl())
                .bg(alpha(gpui_kit::rgb(0xfe9a00), 0.1))
                .text_xs()
                .line_height(px(16.0))
                .text_color(amber_text(p))
                .child(icon("triangle-alert").size(px(14.0)).mt(px(1.0)))
                .child(div().flex_1().min_w_0().child(capitalized(&info.origin_problem)))
                .into_any_element()
        } else {
            let origin = if hide { HIDDEN_ADDRESS.to_owned() } else { shown(&info.origin) };
            div()
                .text_sm()
                .line_height(px(20.0))
                .child(sentence("instancesettings.federation.knownAs", None, &[("origin", origin.as_str(), false)], p))
                .into_any_element()
        };
        // `grid grid-cols-8 gap-x-3 gap-y-1 font-mono text-xs tracking-wider`.
        let mut groups = div().flex().flex_wrap().gap_x(px(12.0)).gap_y(px(4.0)).font_family("monospace").text_xs();
        for (n, group) in info.fingerprint.split(' ').enumerate() {
            groups = groups.child(motion::rise(
                div().child(group.to_owned()),
                SharedString::from(format!("federation-print-{n}-{group}")),
                Duration::from_millis(30 * n as u64),
                4.0,
            ));
        }
        let rotated = match info.rotated_at.as_ref() {
            Some(at) => t_with(
                "instancesettings.federation.lastRotated",
                &[("when", Arg::Str(&ago(ms_of(Some(at)), now_ms())))],
            ),
            None => t("instancesettings.federation.neverRotated"),
        };
        let can_rotate = !info.origin.is_empty();
        let rotate = button(
            "federation-rotate",
            t("instancesettings.federation.rotate"),
            Some("refresh-cw"),
            Look::Outline,
            true,
            p,
        )
        .rounded(radius_xl())
        .when(!can_rotate, |el| el.opacity(0.5))
        .when(can_rotate, |el| {
            el.on_click(cx.listener(|this, _, _, cx| {
                this.federation.rotating = true;
                this.federation.rotate_error = None;
                cx.notify();
            }))
        });
        motion::once(
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
                        .rounded(radius_xl())
                        .bg(alpha(p.muted, 0.6))
                        .child(icon("fingerprint-pattern").size(px(20.0)).text_color(p.primary))
                        .child(div().flex_1().min_w_0().child(groups)),
                )
                .child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t("instancesettings.federation.compare")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(12.0))
                        .child(rotate)
                        .child(div().text_xs().text_color(p.muted_foreground).child(rotated)),
                ),
            "federation-identity",
            Duration::from_millis(300),
            |el, t| el.opacity(t),
        )
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
        let glyph = if f.checking { None } else { Some("radar") };
        let go = button("federation-check-go", t("instancesettings.federation.check"), glyph, Look::Primary, false, p)
            .rounded(radius_xl())
            .when(f.checking, |el| el.child(spinner("federation-checking", 16.0, window)))
            .when(!ready, |el| el.opacity(0.5))
            .when(ready, |el| el.on_click(cx.listener(|this, _, window, cx| this.check_instance(window, cx))));
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div().flex_1().min_w_0().when(!on, |el| el.opacity(0.5)).child(
                            focus_ring(
                                input_box(Input::new(&f.address).appearance(false).disabled(!on), None, p),
                                has_focus(&f.address, window, cx),
                                p,
                            )
                            .h(px(36.0))
                            .font_family("monospace")
                            .text_xs(),
                        ),
                    )
                    .child(go),
            )
            .when(!on, |el| {
                let setting = t("instancesettings.federation.label");
                el.child(
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(p.muted_foreground)
                        .child(t_with("instancesettings.federation.turnOnFirst", &[("setting", Arg::Str(&setting))])),
                )
            });
        if let Some(result) = &f.result {
            let (tint, bg, glyph, text, key): (Hsla, Hsla, &str, gpui_kit::StyledText, String) = match result {
                Ok(r) => {
                    let peer = r.peer.clone().unwrap_or_default();
                    let host = if hide { HIDDEN_ADDRESS.to_owned() } else { shown(&peer.origin) };
                    let ms = r.round_trip_ms.to_string();
                    let reached = t_with(
                        "instancesettings.federation.reached",
                        &[
                            ("origin", Arg::Str(&host)),
                            ("ms", Arg::Str(&ms)),
                            ("fingerprint", Arg::Str(&peer.fingerprint)),
                        ],
                    );
                    let both = t(if r.known_there {
                        "instancesettings.federation.knownThere"
                    } else {
                        "instancesettings.federation.notKnownThere"
                    });
                    (
                        emerald(p),
                        alpha(gpui_kit::rgb(0x00bc7d), 0.1),
                        "check",
                        gpui_kit::StyledText::new(format!("{reached} {both}")),
                        format!("ok-{}", peer.origin),
                    )
                }
                Err(message) => (
                    p.destructive.into(),
                    alpha(p.destructive, 0.1),
                    "triangle-alert",
                    gpui_kit::StyledText::new(capitalized(message)),
                    format!("error-{message}"),
                ),
            };
            card = card.child(motion::rise(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(radius_xl())
                    .bg(bg)
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(tint)
                    .child(icon(glyph).size(px(14.0)).mt(px(1.0)))
                    .child(div().flex_1().min_w_0().child(text)),
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
                .line_height(px(16.0))
                .text_color(p.muted_foreground)
                .child(t("instancesettings.federation.noPeers"))
                .into_any_element();
        }
        let now = now_ms();
        let amber_bg = gpui_kit::rgb(0xfe9a00);
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, peer) in peers.iter().take(f.shown).enumerate() {
            let heard = match &peer.last_heard {
                Some(at) => {
                    t_with("instancesettings.federation.heard", &[("when", Arg::Str(&ago(ms_of(Some(at)), now)))])
                }
                None => t("instancesettings.federation.heardNever"),
            };
            let (edge, bg): (Hsla, Option<Hsla>) = if peer.blocked {
                (alpha(p.destructive, 0.4), Some(alpha(p.destructive, 0.05)))
            } else if peer.needs_check {
                (alpha(amber_bg, 0.4), Some(alpha(amber_bg, 0.05)))
            } else {
                (p.border.into(), None)
            };
            let tag = |glyph: &'static str, text: String, fg: Hsla, bg: Hsla| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.0))
                    .rounded_full()
                    .bg(bg)
                    .px(px(8.0))
                    .py(px(2.0))
                    .text_size(px(10.0))
                    .line_height(px(14.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(fg)
                    .child(icon(glyph).size(px(12.0)))
                    .child(text)
            };
            let last_move = peer.moves.last().map(|m| {
                let when = m.moved_at.as_ref().map(|at| ago(ms_of(Some(at)), now)).unwrap_or_default();
                let start = format!("{}…", m.previous_fingerprint.split(' ').take(2).collect::<Vec<_>>().join(" "));
                let many = peer.moves.len() > 1;
                sentence(
                    if many { "instancesettings.federation.movedMany" } else { "instancesettings.federation.moved" },
                    many.then_some(peer.moves.len() as i64),
                    &[("when", when.as_str(), true), ("fingerprint", start.as_str(), true)],
                    p,
                )
            });
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(12.0))
                    .rounded(radius_xl())
                    .border_1()
                    .border_color(edge)
                    .when_some(bg, |el, bg| el.bg(bg))
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
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(if hide { HIDDEN_ADDRESS.to_owned() } else { shown(&peer.origin) })
                                    .when(peer.blocked, |el| {
                                        el.child(tag(
                                            "ban",
                                            t("instancesettings.federation.blocked"),
                                            p.destructive.into(),
                                            alpha(p.destructive, 0.15),
                                        ))
                                    })
                                    .when(peer.needs_check, |el| {
                                        el.child(tag(
                                            "shield-alert",
                                            t("instancesettings.federation.checkAgain"),
                                            amber_text(p),
                                            alpha(amber_bg, 0.15),
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
                                    .overflow_hidden()
                                    .child(peer.fingerprint.clone()),
                            )
                            .when(peer.needs_check, |el| {
                                el.child(
                                    div()
                                        .mt(px(4.0))
                                        .text_xs()
                                        .line_height(px(16.0))
                                        .text_color(amber_text(p))
                                        .child(t("instancesettings.federation.needsCheck")),
                                )
                            })
                            .when_some(last_move, |el, moved| {
                                el.child(
                                    div()
                                        .mt(px(4.0))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .text_size(px(11.0))
                                        .text_color(p.muted_foreground)
                                        .child(icon("key-round").size(px(12.0)))
                                        .child(
                                            div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(moved),
                                        ),
                                )
                            }),
                    )
                    .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(heard)),
                SharedString::from(format!("federation-peer-{}", peer.origin)),
                Duration::from_millis(30 * n.min(10) as u64),
                6.0,
            ));
        }
        if peers.len() > f.shown {
            let more = (peers.len() - f.shown).min(PAGE) as i64;
            list = list.child(
                div().child(
                    button(
                        "federation-more",
                        t_with("instancesettings.federation.showMore", &[("count", Arg::Num(more))]),
                        None,
                        Look::Ghost,
                        true,
                        p,
                    )
                    .rounded_full()
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
