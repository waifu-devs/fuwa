//! Google, X and Twitch: whether newcomers get accounts by signing in with
//! them, and the app each provider gave this instance (the web's
//! `settings/instance/SignInProviders.tsx`). The instance talks to the
//! provider itself; apps never load anything from it.

use crate::ui::instance_home::{focus_ring, has_focus};
use std::collections::HashMap;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use super::controls::{Opt, amber_soft, input_box, switch};
use super::{HIDDEN_ADDRESS, InstanceSettingsView};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{SIGN_IN_PROVIDERS, provider_accounts, sign_in_setting};
use crate::pb;
use crate::ui::motion;
use crate::ui::theme::{Palette, alpha, radius_xl};
use crate::ui::widgets::icon;

/// The client ID and secret boxes of each provider, by its id.
pub(super) struct Providers {
    ids: HashMap<&'static str, Entity<InputState>>,
    secrets: HashMap<&'static str, Entity<InputState>>,
    /// What each secret box says while it's empty, as last set.
    placeholders: HashMap<&'static str, String>,
}

impl Providers {
    pub(super) fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let mut subs = Vec::new();
        let (mut ids, mut secrets) = (HashMap::new(), HashMap::new());
        for (id, _, _) in SIGN_IN_PROVIDERS {
            let client = cx.new(|cx| InputState::new(window, cx));
            let secret = cx.new(|cx| InputState::new(window, cx).masked(true));
            subs.push(cx.subscribe(&client, move |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    this.edit_provider(id, cx, move |p| p.client_id = value);
                }
            }));
            subs.push(cx.subscribe(&secret, move |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    this.edit_provider(id, cx, move |p| p.client_secret = value);
                }
            }));
            ids.insert(id, client);
            secrets.insert(id, secret);
        }
        (Self { ids, secrets, placeholders: HashMap::new() }, subs)
    }
}

/// "Open", "Linked only" or "Off", for the provider accounts choice.
fn accounts_label(v: i32) -> String {
    if v == pb::ProviderAccounts::Closed as i32 {
        t("instancesettings.providers.closed")
    } else if v == pb::ProviderAccounts::Off as i32 {
        t("serversettings.shared.off")
    } else {
        t("instancesettings.providers.open")
    }
}

impl InstanceSettingsView {
    /// Changes one provider's settings, writing all three so the instance keeps them in order.
    fn edit_provider(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut pb::SignInProviderSetting)) {
        let Some(draft) = self.draft.as_ref() else { return };
        let mut next: Vec<pb::SignInProviderSetting> =
            SIGN_IN_PROVIDERS.iter().map(|(p, _, _)| sign_in_setting(draft, p)).collect();
        let Some(one) = next.iter_mut().find(|p| p.id == id) else { return };
        let before = one.clone();
        f(one);
        if *one != before {
            self.patch(cx, |d| d.sign_in_providers = next);
        }
    }

    /// Puts the draft's client IDs and secrets into their boxes where they say something else
    /// (after loading, a save or a discard).
    pub(super) fn sync_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else { return };
        for (id, _, _) in SIGN_IN_PROVIDERS {
            let setting = sign_in_setting(&draft, id);
            if let Some(state) = self.providers.ids.get(id)
                && state.read(cx).value().as_ref() != setting.client_id
            {
                state.update(cx, |s, cx| s.set_value(setting.client_id.clone(), window, cx));
            }
            if let Some(state) = self.providers.secrets.get(id)
                && state.read(cx).value().as_ref() != setting.client_secret
            {
                state.update(cx, |s, cx| s.set_value(setting.client_secret.clone(), window, cx));
            }
        }
        self.provider_placeholders(window, cx);
    }

    /// The secret boxes say whether a secret is kept for the client ID typed.
    fn provider_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else { return };
        let saved = self.saved().cloned().unwrap_or_default();
        for (id, name, _) in SIGN_IN_PROVIDERS {
            let setting = sign_in_setting(&draft, id);
            let was = sign_in_setting(&saved, id);
            let keeps = was.client_secret_set && setting.client_id.trim() == was.client_id.trim();
            let placeholder = match (keeps, was.client_secret_hint.as_str()) {
                (false, _) => t_with("instancesettings.providers.pasteSecret", &[("name", Arg::Str(name))]),
                (true, "") => t("instancesettings.shared.saved"),
                (true, hint) => t_with("instancesettings.shared.savedEnding", &[("hint", Arg::Str(hint))]),
            };
            if self.providers.placeholders.get(id) == Some(&placeholder) {
                continue;
            }
            if let Some(state) = self.providers.secrets.get(id) {
                state.update(cx, |s, cx| s.set_placeholder(placeholder.clone(), window, cx));
            }
            self.providers.placeholders.insert(id, placeholder);
        }
    }

    pub(super) fn providers_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        self.provider_placeholders(window, cx);
        let saved = self.saved().cloned().unwrap_or_default();
        let defaults = config.defaults.clone().unwrap_or_default();
        let accounts = provider_accounts(&draft);
        let off = accounts == pb::ProviderAccounts::Off as i32;
        let base = saved.public_url.trim().trim_end_matches('/').to_owned();
        let mut page = div().flex().flex_col();
        let choice = self.choice(
            "provider-accounts",
            accounts,
            vec![
                Opt::new(
                    pb::ProviderAccounts::Open as i32,
                    accounts_label(pb::ProviderAccounts::Open as i32),
                    t("instancesettings.providers.openHint"),
                    "door-open",
                ),
                Opt::new(
                    pb::ProviderAccounts::Closed as i32,
                    accounts_label(pb::ProviderAccounts::Closed as i32),
                    t("instancesettings.providers.closedHint"),
                    "door-closed",
                ),
                Opt::new(
                    pb::ProviderAccounts::Off as i32,
                    accounts_label(pb::ProviderAccounts::Off as i32),
                    t("instancesettings.providers.offHint"),
                    "power-off",
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.provider_accounts = v),
        );
        page = page.child(self.setting(
            "provider-accounts",
            &t("instancesettings.nav.providerAccounts"),
            Some(&t("instancesettings.providers.accountsHint")),
            &["provider_accounts"],
            &accounts_label(provider_accounts(&defaults)),
            0,
            choice,
            p,
            cx,
        ));
        let hide = self.core.prefs().streamer_mode;
        for (n, (id, name, console)) in SIGN_IN_PROVIDERS.into_iter().enumerate() {
            let setting = sign_in_setting(&draft, id);
            let was = sign_in_setting(&saved, id);
            let enabled = setting.enabled;
            let head =
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .when(off, |el| el.opacity(0.6))
                    .child(
                        div()
                            .flex_none()
                            .size(px(40.0))
                            .rounded(radius_xl())
                            .bg(p.foreground)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(mark(id, p)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("iprovider-{id}-toggle")))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(16.0))
                            .cursor_pointer()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.edit_provider(id, cx, |p| p.enabled = !enabled)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .child(div().text_sm().line_height(px(20.0)).font_weight(FontWeight::BOLD).child(
                                        t_with("instancesettings.providers.enable", &[("name", Arg::Str(name))]),
                                    ))
                                    .when(off, |el| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .line_height(px(16.0))
                                                .text_color(p.muted_foreground)
                                                .child(t("instancesettings.providers.allOff")),
                                        )
                                    }),
                            )
                            .child(switch(
                                SharedString::from(format!("iprovider-{id}-switch")),
                                enabled,
                                false,
                                p,
                                window,
                                cx,
                                move |this, on, _, cx| this.edit_provider(id, cx, |p| p.enabled = on),
                            )),
                    );
            let mut body = div().flex().flex_col().gap(px(12.0)).child(head);
            if enabled {
                let keeps = was.client_secret_set && setting.client_id.trim() == was.client_id.trim();
                let id_missing = setting.client_id.trim().is_empty();
                let secret_missing = setting.client_secret.trim().is_empty() && !keeps;
                let host = console.split('/').nth(2).unwrap_or(console).to_owned();
                let link = console.to_owned();
                let mut fields = div().flex().flex_col().gap(px(12.0)).child(
                    div()
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(p.muted_foreground)
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .child(format!(
                            "{} ",
                            t_with("instancesettings.providers.makeApp", &[("name", Arg::Str(name))])
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("iprovider-{id}-console")))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.primary)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .on_click(move |_, _, cx| crate::ui::text::open_link(&link, cx))
                                .child(host)
                                .child(icon("external-link").size(px(12.0))),
                        ),
                );
                let note = match id {
                    "x" => Some(t("instancesettings.providers.xScopes")),
                    "twitch" => Some(t("instancesettings.providers.twitchState")),
                    _ => None,
                };
                if let Some(note) = note {
                    fields = fields.child(
                        div()
                            .rounded(radius_xl())
                            .bg(alpha(p.muted, 0.6))
                            .px(px(12.0))
                            .py(px(8.0))
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(p.muted_foreground)
                            .child(note),
                    );
                }
                fields = fields.child(if base.is_empty() {
                    div()
                        .text_xs()
                        .line_height(px(16.0))
                        .text_color(amber_soft(p))
                        .child(t("instancesettings.providers.needsAddress"))
                        .into_any_element()
                } else {
                    let value = format!("{base}/sso/instance/providers/{id}");
                    let shown = if hide { HIDDEN_ADDRESS.to_owned() } else { value.clone() };
                    self.copy_row(
                        &format!("iprovider-{id}-redirect"),
                        &t("instancesettings.providers.redirect"),
                        &shown,
                        &value,
                        p,
                        cx,
                    )
                });
                let ring = |el: gpui_kit::Div, missing: bool| {
                    if missing { el.border_2().border_color(gpui_kit::hsla(38.0 / 360.0, 0.92, 0.5, 0.5)) } else { el }
                };
                if let Some(state) = self.providers.ids.get(id) {
                    fields = fields.child(labelled(
                        &t("instancesettings.providers.clientId"),
                        ring(
                            focus_ring(
                                input_box(Input::new(state).appearance(false), None, p),
                                has_focus(state, window, cx),
                                p,
                            )
                            .font_family("monospace"),
                            id_missing,
                        ),
                        p,
                    ));
                }
                if let Some(state) = self.providers.secrets.get(id) {
                    fields = fields.child(labelled(
                        &t("instancesettings.providers.clientSecret"),
                        ring(
                            focus_ring(
                                input_box(Input::new(state).appearance(false), Some("key-round"), p),
                                has_focus(state, window, cx),
                                p,
                            )
                            .font_family("monospace"),
                            secret_missing,
                        ),
                        p,
                    ));
                }
                if id_missing || secret_missing {
                    fields = fields.child(
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(amber_soft(p))
                            .child(t("instancesettings.providers.missing")),
                    );
                }
                body = body.child(motion::rise(
                    fields,
                    SharedString::from(format!("iprovider-{id}-app")),
                    std::time::Duration::ZERO,
                    -6.0,
                ));
            }
            page = page.child(
                self.setting_with(
                    match id {
                        "google" => "provider-google",
                        "x" => "provider-x",
                        _ => "provider-twitch",
                    },
                    name,
                    Some(
                        div()
                            .child(t_with("instancesettings.providers.providerHint", &[("name", Arg::Str(name))]))
                            .into_any_element(),
                    ),
                    &["sign_in_providers"],
                    &t("serversettings.shared.off"),
                    n + 1,
                    body,
                    p,
                    cx,
                ),
            );
        }
        page.into_any_element()
    }
}

/// A small uppercase label over a field (`text-[0.7rem] font-bold uppercase`).
fn labelled(label: &str, field: impl gpui_kit::IntoElement, p: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(11.2))
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(p.muted_foreground)
                .child(label.to_uppercase()),
        )
        .child(field)
}

/// A provider's mark in the theme's primary, on a dark tile.
fn mark(id: &str, p: &Palette) -> AnyElement {
    gpui_kit::svg()
        .path(SharedString::from(format!("providers/{id}.svg")))
        .size(px(18.0))
        .flex_none()
        .text_color(p.primary)
        .into_any_element()
}
