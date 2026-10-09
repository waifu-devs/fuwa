//! How you get into your account on the instance on screen, as the web's
//! `Security.tsx` (two-step sign-in: four steps to set it up with a QR code
//! and a code box, then backup codes and turning it off, each asking for
//! the password again), `SignInMethods.tsx` (linking Google, X or Twitch
//! through the browser, and unlinking) and `Account.tsx`'s `LinkedSignIn`.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};

use crate::core::i18n::{Arg, t, t_with};
use crate::core::qr::Qr;
use crate::pb;
use crate::ui::motion;
use crate::ui::settings::SettingsView;
use crate::ui::settings_controls::{At, Look, button, field, hint, warn};
use crate::ui::text::{WIDER, tracked};
use crate::ui::theme::{Palette, alpha, radius_2xl, radius_3xl, radius_lg, radius_xl};
use crate::ui::widgets::icon;

const GREEN: u32 = 0x10b981;

/// What the pages keep.
#[derive(Default)]
pub(crate) struct SecurityForm {
    loaded: Option<String>,
    status: Option<pb::GetTwoFactorResponse>,
    error: Option<String>,
    /// Setting up: the step, the secret, and the codes once on.
    setup: Option<u8>,
    secret: Option<pb::SetUpTwoFactorResponse>,
    codes: Vec<String>,
    revealed: bool,
    password: Option<Entity<InputState>>,
    code: Option<Entity<InputState>>,
    digits: Option<Entity<InputState>>,
    busy: bool,
    step_error: Option<String>,
    shake: Option<std::time::Instant>,
    /// Once on: "codes" or "off" while confirming, and new codes to show.
    confirming: Option<&'static str>,
    new_codes: Option<Vec<String>>,
    /// Ways to sign in.
    methods: Option<pb::ListSignInMethodsResponse>,
    methods_for: Option<String>,
    asking: Option<(bool, String, String)>,
}

fn input(window: &mut Window, cx: &mut Context<SettingsView>, masked: bool, placeholder: &str) -> Entity<InputState> {
    let placeholder = placeholder.to_owned();
    let state = cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(placeholder));
    cx.subscribe(&state, |_, _, _: &InputEvent, cx| cx.notify()).detach();
    state
}

fn read(state: &Option<Entity<InputState>>, cx: &Context<SettingsView>) -> String {
    state.as_ref().map(|s| s.read(cx).value().to_string()).unwrap_or_default()
}

fn shaking(el: impl IntoElement + gpui_kit::Styled + 'static, at: Option<std::time::Instant>, id: &str) -> AnyElement {
    match at {
        Some(at) => {
            motion::once(el, SharedString::from(format!("{id}-{at:?}")), Duration::from_millis(400), |el, t| {
                el.relative().left(px((t * std::f32::consts::TAU * 2.5).sin() * 8.0 * (1.0 - t)))
            })
        }
        None => el.into_any_element(),
    }
}

/// A big round badge: the shield, the check.
fn badge(glyph: &'static str, bg: gpui_kit::Hsla, fg: gpui_kit::Hsla, size: f32, round: bool) -> gpui_kit::Div {
    div()
        .size(px(size))
        .flex_none()
        .when(round, |el| el.rounded_full())
        .when(!round, |el| el.rounded(radius_2xl()))
        .bg(bg)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph).size(px(size / 2.0)))
}

impl SettingsView {
    fn load_two_factor(&mut self, key: &str, cx: &mut Context<Self>) {
        let (core, k) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.two_factor(&k).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(s) => {
                        this.security.status = Some(s);
                        this.security.error = None;
                    }
                    Err(e) => this.security.error = Some(e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn security_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        if self.security.loaded.as_deref() != Some(key.as_str()) {
            self.security = SecurityForm { loaded: Some(key.clone()), ..Default::default() };
            self.load_two_factor(&key, cx);
        }
        let place = self.place(&key);
        if self.security.setup.is_some() {
            return self.set_up(&key, &place, p, window, cx);
        }
        let Some(status) = self.security.status else {
            return match self.security.error.clone() {
                Some(e) => div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(alpha(p.destructive, 0.4))
                    .bg(alpha(p.destructive, 0.05))
                    .p(px(16.0))
                    .child(warn(e, p))
                    .child(
                        button("tf-retry", t("accountsettings.shared.tryAgain"), None, Look::Outline, true, p)
                            .rounded(radius_xl())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.security.loaded = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
                None => div().h(px(160.0)).rounded(radius_3xl()).bg(p.muted).into_any_element(),
            };
        };
        if !status.enabled {
            let card = div()
                .relative()
                .overflow_hidden()
                .rounded(radius_3xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .p(px(24.0))
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .child(badge("shield", p.muted.into(), p.muted_foreground.into(), 56.0, false))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::EXTRA_BOLD)
                                        .child(t("accountsettings.security.off")),
                                )
                                .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                                    "accountsettings.security.offHint",
                                    &[("instance", Arg::Str(&place))],
                                ))),
                        )
                        .child(
                            button("tf-on", t("accountsettings.security.turnOn"), None, Look::Primary, false, p)
                                .rounded(radius_xl())
                                .px(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.security.setup = Some(0);
                                    let field = input(window, cx, true, "");
                                    field.update(cx, |s, cx| s.focus(window, cx));
                                    this.security.password = Some(field);
                                    this.security.step_error = None;
                                    cx.notify();
                                })),
                        ),
                );
            return self.found_mark("two-step", div().child(motion::rise(card, "tf-off", Duration::ZERO, 10.0)), p);
        }
        // On.
        let left = status.backup_codes_left;
        let low = left <= 3;
        let card =
            div()
                .relative()
                .overflow_hidden()
                .rounded(radius_3xl())
                .border_1()
                .border_color(alpha(rgb(GREEN), 0.3))
                .bg(alpha(rgb(GREEN), 0.05))
                .p(px(24.0))
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .child(badge("shield-check", rgb(GREEN).into(), rgb(0xffffff).into(), 56.0, false))
                        .child(
                            div()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::EXTRA_BOLD)
                                        .child(t("accountsettings.security.on")),
                                )
                                .child(div().text_sm().text_color(p.muted_foreground).child(t_with(
                                    "accountsettings.security.onHint",
                                    &[("instance", Arg::Str(&place))],
                                ))),
                        ),
                );
        let codes_hint = div()
            .text_sm()
            .text_color(p.muted_foreground)
            .flex()
            .flex_wrap()
            .gap_x(px(4.0))
            .child(
                div().font_weight(FontWeight::BOLD).text_color(if low { p.destructive } else { p.foreground }).child(
                    t_with(
                        "accountsettings.security.codesLeft",
                        &[("count", Arg::Num(i64::from(left))), ("total", Arg::Num(10))],
                    ),
                ),
            )
            .child(if low { t("accountsettings.security.codesLow") } else { t("accountsettings.security.codesEach") })
            .into_any_element();
        let codes_body: AnyElement = if let Some(codes) = self.security.new_codes.clone() {
            motion::rise(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(hint(t("accountsettings.security.oldCodes"), p))
                    .child(self.backup_codes(&codes, &place, p, cx))
                    .child(
                        div().flex().child(
                            button("tf-codes-done", t("accountsettings.shared.done"), None, Look::Outline, true, p)
                                .rounded(radius_xl())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.security.new_codes = None;
                                    cx.notify();
                                })),
                        ),
                    ),
                "tf-codes-new",
                Duration::ZERO,
                8.0,
            )
            .into_any_element()
        } else if self.security.confirming == Some("codes") {
            self.confirm(&key, false, p, cx)
        } else {
            button(
                "tf-make-codes",
                t("accountsettings.security.makeNewCodes"),
                Some("refresh-cw"),
                Look::Outline,
                false,
                p,
            )
            .rounded(radius_xl())
            .on_click(cx.listener(|this, _, window, cx| this.ask(Some("codes"), window, cx)))
            .into_any_element()
        };
        let off_body: AnyElement = if self.security.confirming == Some("off") {
            self.confirm(&key, true, p, cx)
        } else {
            button("tf-off", t("accountsettings.shared.turnOff"), Some("shield-off"), Look::DangerOutline, false, p)
                .rounded(radius_xl())
                .on_click(cx.listener(|this, _, window, cx| this.ask(Some("off"), window, cx)))
                .into_any_element()
        };
        let rows = div()
            .flex()
            .flex_col()
            .child(self.row(
                "backup-codes",
                &t("settings.nav.backupCodes"),
                Some(codes_hint),
                At::of(0, 2),
                div().flex().child(codes_body),
                p,
            ))
            .child(self.row(
                "turn-off-two-step",
                &t("accountsettings.security.turnOffTitle"),
                Some(hint(t("accountsettings.security.turnOffHint"), p)),
                At::of(1, 2),
                div().flex().child(off_body),
                p,
            ));
        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(self.found_mark("two-step", div().child(motion::rise(card, "tf-on-card", Duration::ZERO, 10.0)), p))
            .child(rows)
            .into_any_element()
    }

    fn ask(&mut self, what: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        self.security.confirming = what;
        self.security.password = Some(input(window, cx, true, ""));
        self.security.code = Some(input(window, cx, false, "123456"));
        self.security.step_error = None;
        cx.notify();
    }

    /// Asks for the password (and a code) before something that weakens the account.
    fn confirm(&mut self, key: &str, danger: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let password = read(&self.security.password, cx);
        let code = read(&self.security.code, cx);
        let ready = !password.is_empty() && (!danger || code.trim().len() >= 6);
        let busy = self.security.busy;
        let label = |glyph: &'static str, text: String| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(icon(glyph).size(px(14.0)))
                .child(text)
        };
        let mut fields = div().flex().flex_col().gap(px(12.0));
        if let Some(s) = &self.security.password {
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(label("key-round", t("accountsettings.shared.yourPassword")))
                    .child(field(Input::new(s).appearance(false), p)),
            );
        }
        if danger && let Some(s) = &self.security.code {
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(label("smartphone", t("accountsettings.shared.codeOrBackup")))
                    .child(field(Input::new(s).appearance(false), p)),
            );
        }
        let k = key.to_owned();
        let action = if danger { t("accountsettings.shared.turnOff") } else { t("accountsettings.security.newCodes") };
        motion::rise(
            div()
                .w(px(448.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(p.card)
                .p(px(16.0))
                .child(shaking(fields, self.security.shake, "tf-confirm"))
                .when_some(self.security.step_error.clone(), |el, e| el.child(warn(e, p)))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            button("tf-cancel", t("common.cancel"), None, Look::Ghost, true, p)
                                .rounded(radius_xl())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.security.confirming = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            button(
                                "tf-go",
                                if busy { t("accountsettings.shared.checking") } else { action },
                                None,
                                if danger { Look::Destructive } else { Look::Primary },
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if busy {
                                    return;
                                }
                                if !ready {
                                    this.security.shake = Some(std::time::Instant::now());
                                    cx.notify();
                                    return;
                                }
                                this.security.busy = true;
                                this.security.step_error = None;
                                let (core, k, pw, c) =
                                    (this.core.clone(), k.clone(), password.clone(), code.trim().to_owned());
                                let rx = this.core.spawn(async move {
                                    if danger {
                                        core.disable_two_factor(&k, &pw, &c).await.map(|_| None)
                                    } else {
                                        core.new_backup_codes(&k, &pw).await.map(Some)
                                    }
                                });
                                cx.spawn(async move |this, cx| {
                                    let Ok(result) = rx.await else { return };
                                    let _ = this.update(cx, |this, cx| {
                                        this.security.busy = false;
                                        match result {
                                            Ok(Some(codes)) => {
                                                if let Some(s) = this.security.status.as_mut() {
                                                    s.backup_codes_left = codes.len() as i32;
                                                }
                                                this.security.new_codes = Some(codes);
                                                this.security.confirming = None;
                                            }
                                            Ok(None) => {
                                                this.security.status = Some(pb::GetTwoFactorResponse {
                                                    enabled: false,
                                                    backup_codes_left: 0,
                                                });
                                                this.security.confirming = None;
                                                this.toast("shield-off", t("accountsettings.security.off"), cx);
                                            }
                                            Err(e) => {
                                                this.security.step_error = Some(e.message);
                                                this.security.shake = Some(std::time::Instant::now());
                                            }
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                                cx.notify();
                            })),
                        ),
                ),
            "tf-confirm-in",
            Duration::ZERO,
            8.0,
        )
        .into_any_element()
    }

    /// Setting it up: confirm it's you, scan, type a code, save the backup codes.
    fn set_up(
        &mut self,
        key: &str,
        place: &str,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let step = self.security.setup.unwrap_or(0);
        let busy = self.security.busy;
        let k = key.to_owned();
        let body: AnyElement = match step {
            0 => {
                let password = read(&self.security.password, cx);
                let hint_el = match &self.security.step_error {
                    Some(e) => warn(e.clone(), p),
                    None => hint(t_with("accountsettings.security.passwordHint", &[("instance", Arg::Str(place))]), p),
                };
                let fieldset = self.row(
                    "tf-password",
                    &t("accountsettings.shared.yourPassword"),
                    Some(hint_el),
                    At::of(0, 1),
                    field(Input::new(self.security.password.as_ref().expect("a password field")).appearance(false), p),
                    p,
                );
                div()
                    .w(px(448.0))
                    .child(shaking(div().child(fieldset), self.security.shake, "tf-pw"))
                    .child(
                        div().flex().gap(px(8.0)).child(self.cancel_setup(p, cx)).child(
                            button(
                                "tf-continue",
                                if busy { t("accountsettings.shared.checking") } else { t("common.continue") },
                                None,
                                Look::Primary,
                                false,
                                p,
                            )
                            .rounded(radius_xl())
                            .px(px(20.0))
                            .font_weight(FontWeight::BOLD)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if busy {
                                    return;
                                }
                                if password.is_empty() {
                                    this.security.shake = Some(std::time::Instant::now());
                                    cx.notify();
                                    return;
                                }
                                this.security.busy = true;
                                let (core, k, pw) = (this.core.clone(), k.clone(), password.clone());
                                let rx = this.core.spawn(async move { core.set_up_two_factor(&k, &pw).await });
                                cx.spawn_in(window, async move |this, cx| {
                                    let Ok(result) = rx.await else { return };
                                    let _ = this.update_in(cx, |this, window, cx| {
                                        this.security.busy = false;
                                        match result {
                                            Ok(secret) => {
                                                this.security.secret = Some(secret);
                                                this.security.setup = Some(1);
                                                this.security.step_error = None;
                                                this.security.revealed = false;
                                                if let Some(s) = &this.security.password {
                                                    s.update(cx, |s, cx| s.set_value("", window, cx));
                                                }
                                            }
                                            Err(e) => {
                                                this.security.step_error = Some(e.message);
                                                this.security.shake = Some(std::time::Instant::now());
                                            }
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                                cx.notify();
                            })),
                        ),
                    )
                    .into_any_element()
            }
            1 => {
                let secret = self.security.secret.clone().unwrap_or_default();
                let streaming = self.core.prefs().streamer_mode;
                let veiled = streaming && !self.security.revealed;
                let qr = div()
                    .relative()
                    .flex_none()
                    .child(div().when(veiled, |el| el.opacity(0.08)).child(qr_code(&secret.uri, p)))
                    .when(veiled, |el| {
                        el.child(
                            div()
                                .id("tf-reveal")
                                .absolute()
                                .inset_0()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(px(4.0))
                                .rounded(radius_2xl())
                                .bg(alpha(p.background, 0.6))
                                .p(px(12.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.security.revealed = true;
                                    cx.notify();
                                }))
                                .child(icon("eye").size(px(20.0)))
                                .child(t("accountsettings.security.hidden"))
                                .child(
                                    div()
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(p.muted_foreground)
                                        .child(t("accountsettings.security.showAnyway")),
                                ),
                        )
                    });
                let groups: Vec<String> =
                    secret.secret.chars().collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect()).collect();
                let plain = secret.secret.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(
                        div().flex().items_start().gap(px(24.0)).child(qr).child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::EXTRA_BOLD)
                                                .child(t("accountsettings.security.scan")),
                                        )
                                        .child(hint(t("accountsettings.security.scanHint"), p)),
                                )
                                .child(
                                    div()
                                        .child(
                                            div()
                                                .mb(px(6.0))
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(p.muted_foreground)
                                                .child(t("accountsettings.security.typeKey")),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .rounded(radius_xl())
                                                        .bg(p.muted)
                                                        .px(px(12.0))
                                                        .py(px(8.0))
                                                        .text_sm()
                                                        .font_weight(FontWeight::BOLD)
                                                        .font_family("monospace")
                                                        .when(streaming, |el| el.opacity(0.15))
                                                        .child(groups.join(" ")),
                                                )
                                                .child(
                                                    button("tf-copy-key", "", Some("copy"), Look::Outline, false, p)
                                                        .w(px(36.0))
                                                        .px(px(0.0))
                                                        .rounded(radius_xl())
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                                plain.clone(),
                                                            ));
                                                            this.toast("copy", t("common.copy.key"), cx);
                                                        })),
                                                ),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        div().flex().gap(px(8.0)).child(self.cancel_setup(p, cx)).child(
                            button("tf-added", t("accountsettings.security.added"), None, Look::Primary, false, p)
                                .rounded(radius_xl())
                                .px(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.security.setup = Some(2);
                                    this.security.digits = Some(input(window, cx, false, ""));
                                    if let Some(d) = this.security.digits.clone() {
                                        d.update(cx, |s, cx| s.focus(window, cx));
                                    }
                                    cx.notify();
                                })),
                        ),
                    )
                    .into_any_element()
            }
            2 => {
                let typed: String =
                    read(&self.security.digits, cx).chars().filter(char::is_ascii_digit).take(6).collect();
                if typed.len() == 6 && !busy && self.security.step_error.is_none() {
                    self.verify(key, typed.clone(), window, cx);
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(
                        div()
                            .child(
                                div().font_weight(FontWeight::EXTRA_BOLD).child(t("accountsettings.security.typeCode")),
                            )
                            .child(hint(t("accountsettings.security.typeCodeHint"), p)),
                    )
                    .child(shaking(
                        div().child(self.code_boxes(&typed, busy, p, window, cx)),
                        self.security.shake,
                        "tf-code",
                    ))
                    .when_some(self.security.step_error.clone(), |el, e| el.child(warn(e, p)))
                    .child(div().flex().child(
                        button("tf-back", t("common.back"), None, Look::Ghost, false, p).rounded(radius_xl()).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.security.setup = Some(1);
                                this.security.step_error = None;
                                cx.notify();
                            }),
                        ),
                    ))
                    .into_any_element()
            }
            _ => {
                let codes = self.security.codes.clone();
                let n = codes.len() as i32;
                div()
                    .flex()
                    .flex_col()
                    .gap(px(20.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(badge("shield-check", rgb(GREEN).into(), rgb(0xffffff).into(), 44.0, true))
                            .child(
                                div()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::EXTRA_BOLD)
                                            .child(t("accountsettings.security.on")),
                                    )
                                    .child(hint(t("accountsettings.security.onSetupHint"), p)),
                            ),
                    )
                    .child(self.backup_codes(&codes, place, p, cx))
                    .child(
                        div().flex().child(
                            button("tf-saved", t("accountsettings.security.savedThem"), None, Look::Primary, false, p)
                                .rounded(radius_xl())
                                .px(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.security.status =
                                        Some(pb::GetTwoFactorResponse { enabled: true, backup_codes_left: n });
                                    this.security.setup = None;
                                    cx.notify();
                                })),
                        ),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(steps(step, p, window, cx))
            .child(motion::slide_in(div().child(body), SharedString::from(format!("tf-step-{step}")), 24.0))
            .into_any_element()
    }

    fn cancel_setup(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        button("tf-cancel-setup", t("common.cancel"), None, Look::Ghost, false, p)
            .rounded(radius_xl())
            .on_click(cx.listener(|this, _, _, cx| {
                this.security.setup = None;
                this.security.step_error = None;
                cx.notify();
            }))
            .into_any_element()
    }

    fn verify(&mut self, key: &str, code: String, window: &mut Window, cx: &mut Context<Self>) {
        self.security.busy = true;
        let (core, k) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.enable_two_factor(&k, &code).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.security.busy = false;
                match result {
                    Ok(codes) => {
                        this.security.codes = codes;
                        this.security.setup = Some(3);
                        this.security.step_error = None;
                    }
                    Err(e) => {
                        this.security.step_error = Some(e.message);
                        this.security.shake = Some(std::time::Instant::now());
                        if let Some(d) = &this.security.digits {
                            d.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Six boxes for a code (the web's `CodeInput`): one real field under them takes the keys.
    fn code_boxes(
        &mut self,
        typed: &str,
        busy: bool,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(state) = self.security.digits.clone() else { return div().into_any_element() };
        // A new digit after a wrong code starts it over.
        if self.security.step_error.is_some() && !typed.is_empty() {
            self.security.step_error = None;
        }
        let focused = gpui_kit::Focusable::focus_handle(state.read(cx), cx).is_focused(window);
        let caret = typed.len().min(5);
        let digits: Vec<char> = typed.chars().collect();
        let mut boxes = div().flex().gap(px(8.0));
        for n in 0..6 {
            let digit = digits.get(n).copied();
            let here = focused && n == caret && !busy;
            boxes = boxes.child(
                div()
                    .relative()
                    .when(n == 3, |el| el.ml(px(8.0)))
                    .w(px(48.0))
                    .h(px(56.0))
                    .rounded(radius_xl())
                    .border_2()
                    .border_color(if here {
                        p.primary.into()
                    } else if digit.is_some() {
                        alpha(p.primary, 0.5)
                    } else {
                        p.border.into()
                    })
                    .when(here, |el| {
                        el.shadow(vec![gpui_kit::BoxShadow {
                            color: alpha(p.primary, 0.2),
                            offset: gpui_kit::point(px(0.0), px(0.0)),
                            blur_radius: px(0.0),
                            spread_radius: px(4.0),
                            inset: false,
                        }])
                    })
                    .bg(p.background)
                    .when(busy, |el| el.opacity(0.6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_2xl()
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .map(|el| match digit {
                        Some(d) => el.child(motion::rise(
                            div().child(d.to_string()),
                            SharedString::from(format!("digit-{n}-{d}")),
                            Duration::ZERO,
                            8.0,
                        )),
                        None if here => el.child(motion::ambient(
                            div().w(px(2.0)).h(px(24.0)).rounded_full().bg(p.primary),
                            "code-caret",
                            Duration::from_millis(1000),
                            window,
                            |el, t| el.opacity(if t < 0.5 { 1.0 } else { 0.0 }),
                        )),
                        None => el,
                    }),
            );
        }
        div()
            .relative()
            .flex()
            .child(boxes)
            .child(div().absolute().inset_0().opacity(0.0).child(Input::new(&state).appearance(false).h_full()))
            .into_any_element()
    }

    /// Backup codes in a grid, to copy or save as a file.
    fn backup_codes(&self, codes: &[String], place: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let streaming = self.core.prefs().streamer_mode;
        let mut grid = div().flex().flex_wrap().gap(px(8.0)).when(streaming, |el| el.opacity(0.15));
        for (n, code) in codes.iter().enumerate() {
            grid = grid.child(motion::rise(
                div()
                    .w(px(104.0))
                    .rounded(radius_lg())
                    .bg(p.muted)
                    .px(px(8.0))
                    .py(px(6.0))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .font_family("monospace")
                    .child(tracked(code.clone(), WIDER)),
                SharedString::from(format!("code-{n}-{code}")),
                Duration::from_millis(50 + 35 * n as u64),
                10.0,
            ));
        }
        let all = codes.join("\n");
        let file = format!(
            "{}\n{}\n\n{}\n",
            t_with("accountsettings.security.codesFileTitle", &[("instance", Arg::Str(place))]),
            t("accountsettings.security.codesFileNote"),
            all
        );
        let slug: String = place
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c.to_ascii_lowercase() } else { '-' })
            .collect();
        div()
            .rounded(radius_2xl())
            .border_1()
            .border_color(p.border)
            .bg(p.card)
            .p(px(16.0))
            .child(grid)
            .child(
                div()
                    .mt(px(12.0))
                    .flex()
                    .flex_wrap()
                    .gap(px(8.0))
                    .child(
                        button(
                            "codes-copy",
                            t("accountsettings.security.copyAll"),
                            Some("copy"),
                            Look::Outline,
                            true,
                            p,
                        )
                        .rounded(radius_xl())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(all.clone()));
                            this.toast("copy", t("common.copy.backupCodes"), cx);
                        })),
                    )
                    .child(
                        button(
                            "codes-save",
                            t("accountsettings.shared.download"),
                            Some("download"),
                            Look::Outline,
                            true,
                            p,
                        )
                        .rounded(radius_xl())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
                            let path = cx.prompt_for_new_path(&dir, Some(&format!("fuwa-backup-codes-{slug}.txt")));
                            let file = file.clone();
                            cx.spawn(async move |this, cx| {
                                let Ok(Ok(Some(path))) = path.await else { return };
                                let saved = std::fs::write(&path, file).is_ok();
                                let _ = this.update(cx, |this, cx| {
                                    if saved {
                                        this.toast("download", t("accountsettings.security.codesDownloaded"), cx);
                                    }
                                });
                            })
                            .detach();
                        })),
                    ),
            )
            .into_any_element()
    }

    // ───────────────────────── Ways to sign in ─────────────────────────

    fn load_methods(&mut self, key: &str, cx: &mut Context<Self>) {
        self.security.methods_for = Some(key.to_owned());
        let (core, k) = (self.core.clone(), key.to_owned());
        let rx = self.core.spawn(async move { core.sign_in_methods(&k).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(m) => {
                        this.security.methods = Some(m);
                        this.security.error = None;
                    }
                    Err(e) => this.security.error = Some(e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn sign_in_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        if self.security.methods_for.as_deref() != Some(key.as_str()) {
            self.security.methods = None;
            self.load_methods(&key, cx);
        }
        let Some(listed) = self.security.methods.clone() else {
            return match self.security.error.clone() {
                Some(e) => warn(e, p),
                None => icon("loader-circle").size(px(20.0)).text_color(p.muted_foreground).into_any_element(),
            };
        };
        let working = listed.methods.iter().filter(|m| m.works).count();
        let hidden = self.core.prefs().hides_personal();
        let mut list = div().flex().flex_col().gap(px(8.0));
        for (n, m) in listed.methods.iter().enumerate() {
            let only = m.works && working == 1;
            let provider = !["password", "waifu", "sso"].contains(&m.kind.as_str());
            let name = if m.kind == "password" { t("accountsettings.signIn.password") } else { m.name.clone() };
            let glyph = match m.kind.as_str() {
                "password" => "key-round",
                "waifu" => "flower-2",
                "sso" => "building",
                "google" => "brand-google",
                "x" => "brand-x",
                "twitch" => "brand-twitch",
                _ => "log-in",
            };
            let linked = crate::ui::text::ms_of(m.linked_at.as_ref());
            let mut sub = Vec::new();
            if !m.account_name.is_empty() {
                sub.push(if hidden { "••••••".to_owned() } else { m.account_name.clone() });
            }
            if linked > 0 {
                use chrono::TimeZone as _;
                let day = chrono::Local
                    .timestamp_millis_opt(linked)
                    .single()
                    .map(|d| d.format("%b %-d, %Y").to_string())
                    .unwrap_or_default();
                sub.push(t_with("accountsettings.signIn.linkedOn", &[("date", Arg::Str(&day))]));
            }
            let (id, pname) = (m.kind.clone(), m.name.clone());
            list = list.child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .p(px(12.0))
                    .child(badge(glyph, p.foreground.into(), p.primary.into(), 40.0, false).rounded(radius_xl()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(name)
                                    .when(!m.works, |el| {
                                        el.child(
                                            div()
                                                .rounded_full()
                                                .bg(p.muted)
                                                .px(px(8.0))
                                                .py(px(2.0))
                                                .text_size(px(11.0))
                                                .text_color(p.muted_foreground)
                                                .child(t("accountsettings.signIn.off")),
                                        )
                                    }),
                            )
                            .when(!sub.is_empty(), |el| {
                                el.child(
                                    div().text_xs().text_color(p.muted_foreground).truncate().child(sub.join(" · ")),
                                )
                            }),
                    )
                    .when(provider, |el| {
                        el.child(
                            button(
                                SharedString::from(format!("unlink-{}", m.kind)),
                                t("accountsettings.signIn.unlink"),
                                Some("unlink-2"),
                                Look::Ghost,
                                true,
                                p,
                            )
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.destructive)
                            .when(only, |el| el.opacity(0.5))
                            .when(!only, |el| {
                                el.on_click(cx.listener(move |this, _, window, cx| {
                                    this.security.asking = Some((false, id.clone(), pname.clone()));
                                    this.ask(None, window, cx);
                                }))
                            }),
                        )
                    }),
                SharedString::from(format!("method-{}", m.kind)),
                Duration::from_millis(40 * n as u64),
                8.0,
            ));
        }
        let mut page = div().flex().flex_col().gap(px(24.0)).child(
            self.found_mark(
                "sign-in-methods",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("accountsettings.signIn.yours")))
                    .child(list)
                    .when(working == 1, |el| {
                        el.child(
                            div().text_xs().text_color(p.muted_foreground).child(t("accountsettings.signIn.onlyWay")),
                        )
                    }),
                p,
            ),
        );
        if listed.can_link && !listed.available.is_empty() {
            let mut buttons = div().flex().flex_wrap().gap(px(8.0));
            for option in &listed.available {
                let (id, name) = (option.id.clone(), option.name.clone());
                let hover = alpha(p.primary, 0.6);
                buttons = buttons.child(
                    div()
                        .id(SharedString::from(format!("link-{}", option.id)))
                        .group(SharedString::from(format!("link-{}", option.id)))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .rounded(radius_xl())
                        .border_1()
                        .border_color(p.border)
                        .bg(p.card)
                        .px(px(14.0))
                        .py(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(move |s| s.border_color(hover).translate_y(px(-2.0)))
                        .active(|s| s.scale(0.97))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.security.asking = Some((true, id.clone(), name.clone()));
                            this.ask(None, window, cx);
                        }))
                        .child(
                            div()
                                .id("mark")
                                .group_hover(SharedString::from(format!("link-{}", option.id)), |s| s.scale(1.1))
                                .child(icon(&format!("brand-{}", option.id)).size(px(16.0))),
                        )
                        .child(t_with("accountsettings.signIn.link", &[("name", Arg::Str(&option.name))]))
                        .child(icon("link").size(px(14.0)).text_color(p.muted_foreground)),
                );
            }
            page = page.child(
                self.found_mark(
                    "link-provider",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(div().font_weight(FontWeight::EXTRA_BOLD).child(t("accountsettings.signIn.add")))
                        .child(buttons)
                        .child(
                            div().text_xs().text_color(p.muted_foreground).child(t("accountsettings.signIn.addNote")),
                        ),
                    p,
                ),
            );
        }
        if let Some((linking, id, name)) = self.security.asking.clone() {
            page = page.child(self.proof(&key, &listed, linking, &id, &name, p, window, cx));
        }
        page.into_any_element()
    }

    /// The password, two-step code or a fresh sign-in a change asks for, then the change.
    #[allow(clippy::too_many_arguments)]
    fn proof(
        &mut self,
        key: &str,
        listed: &pb::ListSignInMethodsResponse,
        linking: bool,
        id: &str,
        name: &str,
        p: &Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let password = read(&self.security.password, cx);
        let code = read(&self.security.code, cx);
        let ready = (!listed.needs_password || !password.is_empty()) && (!listed.needs_code || !code.trim().is_empty());
        let busy = self.security.busy;
        let label = |text: String| div().text_sm().font_weight(FontWeight::BOLD).child(text);
        let (k, id, name) = (key.to_owned(), id.to_owned(), name.to_owned());
        let title = if linking {
            t_with("accountsettings.signIn.linkTitle", &[("name", Arg::Str(&name))])
        } else {
            t_with("accountsettings.signIn.unlinkTitle", &[("name", Arg::Str(&name))])
        };
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .bg(alpha(p.muted, 0.4))
                .p(px(16.0))
                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title))
                .when(listed.needs_fresh_sign_in, |el| el.child(hint(t("accountsettings.signIn.freshNote"), p)))
                .when(listed.needs_password, |el| {
                    el.when_some(self.security.password.clone(), |el, s| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(label(t("accountsettings.signIn.passwordLabel")))
                                .child(field(Input::new(&s).appearance(false), p)),
                        )
                    })
                })
                .when(listed.needs_code, |el| {
                    el.when_some(self.security.code.clone(), |el, s| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(label(t("accountsettings.signIn.codeLabel")))
                                .child(field(Input::new(&s).appearance(false), p)),
                        )
                    })
                })
                .when_some(self.security.step_error.clone(), |el, e| el.child(warn(e, p)))
                .child(
                    div()
                        .flex()
                        .flex_row_reverse()
                        .gap(px(8.0))
                        .child(
                            button(
                                "proof-go",
                                if linking {
                                    t_with("accountsettings.signIn.continueTo", &[("name", Arg::Str(&name))])
                                } else {
                                    t("accountsettings.signIn.unlink")
                                },
                                Some(if busy {
                                    "loader-circle"
                                } else if linking {
                                    "link"
                                } else {
                                    "unlink-2"
                                }),
                                if linking { Look::Primary } else { Look::Destructive },
                                false,
                                p,
                            )
                            .h(px(40.0))
                            .rounded(radius_xl())
                            .font_weight(FontWeight::BOLD)
                            .when(!ready || busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !ready || busy {
                                    return;
                                }
                                this.security.busy = true;
                                this.security.step_error = None;
                                let (core, k, id, name) = (this.core.clone(), k.clone(), id.clone(), name.clone());
                                let (pw, c) = (password.clone(), code.trim().to_owned());
                                let rx = this.core.spawn({
                                    let core = core.clone();
                                    let k = k.clone();
                                    let id = id.clone();
                                    async move {
                                        if linking {
                                            core.link_provider(&k, &id, &pw, &c, |url| {
                                                let _ = open::that(url);
                                            })
                                            .await
                                            .map(|_| ())
                                        } else {
                                            core.unlink_provider(&k, &id, &pw, &c).await
                                        }
                                    }
                                });
                                cx.spawn(async move |this, cx| {
                                    let Ok(result) = rx.await else { return };
                                    let _ = this.update(cx, |this, cx| {
                                        this.security.busy = false;
                                        match result {
                                            Ok(()) => {
                                                if !linking {
                                                    this.toast(
                                                        "unlink-2",
                                                        t_with(
                                                            "accountsettings.signIn.unlinked",
                                                            &[("name", Arg::Str(&name))],
                                                        ),
                                                        cx,
                                                    );
                                                }
                                                this.security.asking = None;
                                                this.security.methods_for = None;
                                            }
                                            Err(e) => this.security.step_error = Some(e.message),
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                                cx.notify();
                            })),
                        )
                        .child(
                            button("proof-cancel", t("common.cancel"), None, Look::Ghost, false, p)
                                .h(px(40.0))
                                .rounded(radius_xl())
                                .font_weight(FontWeight::BOLD)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.security.asking = None;
                                    this.security.step_error = None;
                                    cx.notify();
                                })),
                        ),
                ),
            "proof-in",
            Duration::ZERO,
            12.0,
        )
        .into_any_element()
    }

    /// For a linked account, which has no password here: where its sign-in lives instead.
    pub(crate) fn linked_page(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(key) = self.account_key() else { return div().into_any_element() };
        let issuer = self.core.shared.read(|s| {
            s.instance(&key)
                .and_then(|i| i.node.as_ref())
                .and_then(|n| n.auth.as_ref())
                .map(|a| a.linked_issuer.clone())
                .unwrap_or_default()
        });
        let name = crate::ui::settings::issuer_name(&issuer);
        let place = self.place(&key);
        let waifu = issuer.is_empty() || issuer == "https://api.waifu.dev";
        let lines = [
            ("shield-check", t_with("accountsettings.linked.passwordLives", &[("issuer", Arg::Str(&name))])),
            (
                "monitor-smartphone",
                t_with(
                    "accountsettings.linked.signOut",
                    &[("issuer", Arg::Str(&name)), ("instance", Arg::Str(&place))],
                ),
            ),
            ("user-pen", t_with("accountsettings.linked.picture", &[("issuer", Arg::Str(&name))])),
        ];
        let _ = cx;
        div()
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(motion::rise(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    .rounded(radius_2xl())
                    .border_1()
                    .border_color(p.border)
                    .bg(p.card)
                    .p(px(16.0))
                    .child(badge("flower-2", p.foreground.into(), p.primary.into(), 48.0, false))
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::EXTRA_BOLD)
                                    .child(t_with("accountsettings.linked.title", &[("issuer", Arg::Str(&name))])),
                            )
                            .child(hint(t_with("accountsettings.linked.hint", &[("issuer", Arg::Str(&name))]), p)),
                    ),
                "linked-card",
                Duration::ZERO,
                12.0,
            ))
            .child(div().flex().flex_col().gap(px(10.0)).children(lines.into_iter().enumerate().map(
                |(n, (glyph, line))| {
                    motion::slide_in(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.0))
                            .text_sm()
                            .text_color(p.muted_foreground)
                            .child(icon(glyph).size(px(16.0)).mt(px(2.0)).text_color(p.primary))
                            .child(div().flex_1().child(line)),
                        SharedString::from(format!("linked-line-{n}")),
                        -8.0,
                    )
                },
            )))
            .when(waifu, |el| {
                el.child(
                    div()
                        .id("linked-settings")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.primary)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(|_, _, cx| cx.open_url("https://www.waifu.dev/settings"))
                        .child(t("accountsettings.linked.settings"))
                        .child(icon("external-link").size(px(14.0))),
                )
            })
            .into_any_element()
    }
}

/// Where setting up has got to, as a row of dots joined by a filling line.
fn steps(step: u8, p: &Palette, window: &mut Window, cx: &mut Context<SettingsView>) -> AnyElement {
    let labels = [
        t("accountsettings.security.stepConfirm"),
        t("accountsettings.security.stepScan"),
        t("accountsettings.security.stepCode"),
        t("accountsettings.security.stepSave"),
    ];
    let mut row = div().flex().items_center().gap(px(8.0));
    for (n, label) in labels.into_iter().enumerate() {
        let n8 = n as u8;
        let done = n8 < step;
        let here = n8 == step;
        let dot = div()
            .size(px(28.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(FontWeight::EXTRA_BOLD)
            .map(|el| {
                if done {
                    el.bg(p.primary).text_color(p.primary_foreground)
                } else if here {
                    el.bg(alpha(p.primary, 0.15)).text_color(p.primary).border_2().border_color(p.primary)
                } else {
                    el.bg(p.muted).text_color(p.muted_foreground)
                }
            })
            .child(if done {
                icon("check").size(px(14.0)).into_any_element()
            } else {
                div().child((n + 1).to_string()).into_any_element()
            });
        row = row.child(
            div().flex().items_center().gap(px(8.0)).child(dot).child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .whitespace_nowrap()
                    .text_color(if here { p.foreground } else { p.muted_foreground })
                    .child(label),
            ),
        );
        if n < 3 {
            let fill =
                motion::follow(SharedString::from(format!("tf-line-{n}")), if done { 1.0 } else { 0.0 }, window, cx);
            row = row.child(
                div()
                    .flex_1()
                    .min_w(px(16.0))
                    .h(px(2.0))
                    .rounded_full()
                    .bg(p.muted)
                    .overflow_hidden()
                    .child(div().h_full().w(gpui_kit::relative(fill.clamp(0.0, 1.0))).bg(p.primary)),
            );
        }
    }
    row.into_any_element()
}

/// A QR code as soft dots with rounded corner marks, popping in as a wave (the web's `QrCode`).
fn qr_code(text: &str, p: &Palette) -> AnyElement {
    let Some(qr) = Qr::encode(text) else { return div().into_any_element() };
    let n = qr.size;
    let side = 192.0;
    let cell = side / (n as f32 + 2.0);
    let ink = rgb(0x111118);
    let finder = crate::ui::theme::mix(p.primary, rgb(0x000000), 0.55);
    let in_finder = |x: usize, y: usize| (x < 7 && y < 7) || (x >= n - 7 && y < 7) || (x < 7 && y >= n - 7);
    let mut code = div().relative().size(px(side));
    for y in 0..n {
        for x in 0..n {
            if qr.get(x, y) && !in_finder(x, y) {
                let at = (x + y) as u64 * 10;
                code = code.child(motion::once(
                    div()
                        .absolute()
                        .left(px((x as f32 + 1.0 + 0.08) * cell))
                        .top(px((y as f32 + 1.0 + 0.08) * cell))
                        .size(px(cell * 0.84))
                        .rounded(px(cell * 0.32))
                        .bg(ink),
                    SharedString::from(format!("qr-{x}-{y}")),
                    Duration::from_millis(at + 450),
                    move |el, t| {
                        let start = at as f32 / (at as f32 + 450.0);
                        let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                        el.opacity(k)
                    },
                ));
            }
        }
    }
    for (fx, fy) in [(0usize, 0usize), (n - 7, 0), (0, n - 7)] {
        code = code
            .child(
                div()
                    .absolute()
                    .left(px((fx as f32 + 1.5) * cell))
                    .top(px((fy as f32 + 1.5) * cell))
                    .size(px(6.0 * cell))
                    .rounded(px(1.8 * cell))
                    .border(px(cell))
                    .border_color(finder),
            )
            .child(
                div()
                    .absolute()
                    .left(px((fx as f32 + 3.0) * cell))
                    .top(px((fy as f32 + 3.0) * cell))
                    .size(px(3.0 * cell))
                    .rounded(px(0.9 * cell))
                    .bg(finder),
            );
    }
    div()
        .rounded(radius_2xl())
        .bg(rgb(0xffffff))
        .p(px(12.0))
        .border_1()
        .border_color(gpui_kit::hsla(0.0, 0.0, 0.0, 0.05))
        .shadow(crate::ui::settings_controls::shadow_lg())
        .child(code)
        .into_any_element()
}
