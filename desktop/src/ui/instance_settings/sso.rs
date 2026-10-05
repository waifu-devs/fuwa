//! Single sign-on: people sign in here through the organization's identity
//! provider (OpenID Connect or SAML), as on the web's "Single sign-on" page
//! and its `IdentityProviderForm.tsx`: whether such accounts are open, the
//! provider's details, which email domains get in, what to tell the
//! provider, and a test sign-in through the saved one.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, hsla,
    px,
};

use super::controls::Opt;
use super::signups::accounts_label;
use super::{InstanceSettingsView, hidden_address};
use crate::core::i18n::{Arg, t, t_with};
use crate::core::instance_admin::{self as admin};
use crate::core::sso::{self, MAX_DOMAINS};
use crate::pb;
use crate::ui::motion;
use crate::ui::server_settings::spinner;
use crate::ui::theme::{Palette, alpha, corner};
use crate::ui::widgets::{icon, primary_button, soft_button};

type Get = fn(&pb::IdentityProvider) -> String;
type Set = fn(&mut pb::IdentityProvider, String);

/// The provider's one-line boxes: (key, placeholder, masked, read, write). Placeholders in
/// words come from `placeholder`.
const LINES: [(&str, &str, bool, Get, Set); 6] = [
    ("name", "Acme", false, |p| p.name.clone(), |p, v| p.name = v.chars().take(40).collect()),
    (
        "issuer",
        "https://login.acme.com",
        false,
        |p| p.oidc.as_ref().map(|o| o.issuer.clone()).unwrap_or_default(),
        |p, v| p.oidc.get_or_insert_with(Default::default).issuer = v,
    ),
    (
        "client_id",
        "",
        false,
        |p| p.oidc.as_ref().map(|o| o.client_id.clone()).unwrap_or_default(),
        |p, v| p.oidc.get_or_insert_with(Default::default).client_id = v,
    ),
    (
        "client_secret",
        "",
        true,
        |p| p.oidc.as_ref().map(|o| o.client_secret.clone()).unwrap_or_default(),
        |p, v| p.oidc.get_or_insert_with(Default::default).client_secret = v,
    ),
    (
        "scopes",
        "",
        false,
        |p| p.oidc.as_ref().map(|o| o.extra_scopes.clone()).unwrap_or_default(),
        |p, v| p.oidc.get_or_insert_with(Default::default).extra_scopes = v,
    ),
    (
        "entity_id",
        "https://idp.acme.com/metadata",
        false,
        |p| p.saml.as_ref().map(|s| s.entity_id.clone()).unwrap_or_default(),
        |p, v| p.saml.get_or_insert_with(Default::default).entity_id = v,
    ),
];

/// The SAML sign-in URL, kept apart so metadata can light it up with the entity ID.
const SSO_URL: (&str, &str, Get, Set) = (
    "sso_url",
    "https://idp.acme.com/sso/redirect",
    |p| p.saml.as_ref().map(|s| s.sso_url.clone()).unwrap_or_default(),
    |p, v| p.saml.get_or_insert_with(Default::default).sso_url = v,
);

/// A box's placeholder: words for the ones that have them, else the example in `LINES`.
fn placeholder(key: &str, example: &str) -> String {
    match key {
        "client_id" => t("system.sso.clientId"),
        "client_secret" => t("system.sso.clientSecret"),
        "scopes" => t("instancesettings.provider.scopes"),
        _ => example.to_owned(),
    }
}

/// How a test sign-in went.
pub(super) enum Tested {
    Waiting,
    Signed(Option<pb::SsoIdentity>),
    Failed(String),
}

/// The page's boxes and its moments: pasting metadata, a test sign-in.
pub(super) struct Sso {
    lines: Vec<(&'static str, Entity<InputState>)>,
    sso_url: Entity<InputState>,
    certificates: Entity<TextareaState>,
    domain: Entity<InputState>,
    metadata: Entity<TextareaState>,
    pasting: bool,
    metadata_empty: bool,
    /// How many times metadata filled the form, so the fields glow anew.
    filled: u32,
    problem: Option<String>,
    tested: Option<Tested>,
    /// Which "Tell your provider" row was just copied.
    copied: Option<usize>,
}

impl Sso {
    pub(super) fn new(window: &mut Window, cx: &mut Context<InstanceSettingsView>) -> (Self, Vec<Subscription>) {
        let mut subs = Vec::new();
        let mut lines = Vec::new();
        for (key, example, masked, get, set) in LINES {
            let state = cx.new(|cx| {
                let s = InputState::new(window, cx).placeholder(placeholder(key, example));
                if masked { s.masked(true) } else { s }
            });
            subs.push(Self::follow(&state, get, set, cx));
            lines.push((key, state));
        }
        let (_, placeholder, get, set) = SSO_URL;
        let sso_url = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        subs.push(Self::follow(&sso_url, get, set, cx));
        let certificates = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 10)
                .placeholder("-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----")
        });
        subs.push(cx.subscribe(&certificates, |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.patch_provider(cx, |p| {
                    if p.saml.as_ref().is_none_or(|s| s.certificates != value) {
                        p.saml.get_or_insert_with(Default::default).certificates = value;
                    }
                });
            }
        }));
        let domain = cx.new(|cx| InputState::new(window, cx).placeholder("acme.com"));
        subs.push(cx.subscribe_in(
            &domain,
            window,
            |this: &mut InstanceSettingsView, state, e: &InputEvent, window, cx| {
                let text = state.read(cx).value().to_string();
                match e {
                    // Enter, a comma, a space or leaving the box adds what's typed.
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.add_domains(&text, window, cx),
                    InputEvent::Change if text.ends_with([',', ' ']) => this.add_domains(&text, window, cx),
                    _ => {}
                }
            },
        ));
        let metadata = cx.new(|cx| TextareaState::new(window, cx).auto_grow(5, 10).placeholder("<EntityDescriptor …>"));
        subs.push(cx.subscribe(&metadata, |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.sso.metadata_empty = state.read(cx).value().trim().is_empty();
                cx.notify();
            }
        }));
        let sso = Self {
            lines,
            sso_url,
            certificates,
            domain,
            metadata,
            pasting: false,
            metadata_empty: true,
            filled: 0,
            problem: None,
            tested: None,
            copied: None,
        };
        (sso, subs)
    }

    /// Writes a box into the draft's provider as it changes.
    fn follow(state: &Entity<InputState>, get: Get, set: Set, cx: &mut Context<InstanceSettingsView>) -> Subscription {
        cx.subscribe(state, move |this: &mut InstanceSettingsView, state, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.patch_provider(cx, |p| {
                    if get(p) != value {
                        set(p, value);
                    }
                });
            }
        })
    }

    fn line(&self, key: &str) -> Option<&Entity<InputState>> {
        self.lines.iter().find(|(k, _)| *k == key).map(|(_, s)| s)
    }
}

impl InstanceSettingsView {
    /// Changes the draft's provider, with every part of it there to change.
    fn patch_provider(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut pb::IdentityProvider)) {
        let Some(draft) = self.draft.as_ref() else { return };
        let mut next = sso::full_provider(draft.sso_provider.as_ref());
        f(&mut next);
        if Some(&next) != draft.sso_provider.as_ref() {
            self.patch(cx, |d| d.sso_provider = Some(next));
        }
    }

    fn add_domains(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text.trim().is_empty() {
            return;
        }
        self.patch_provider(cx, |p| sso::add_domains(&mut p.email_domains, text));
        self.sso.domain.update(cx, |s, cx| s.set_value("", window, cx));
        self.sync_sso(window, cx);
    }

    /// Puts the draft's provider into the boxes that don't already say it.
    pub(super) fn sync_sso(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else { return };
        let p = sso::full_provider(draft.sso_provider.as_ref());
        for (key, _, _, get, _) in LINES {
            let Some(state) = self.sso.line(key).cloned() else { continue };
            let want = get(&p);
            if state.read(cx).value().as_ref() != want {
                state.update(cx, |s, cx| s.set_value(want, window, cx));
            }
        }
        let (_, _, get, _) = SSO_URL;
        let want = get(&p);
        if self.sso.sso_url.read(cx).value().as_ref() != want {
            self.sso.sso_url.update(cx, |s, cx| s.set_value(want, window, cx));
        }
        let want = p.saml.as_ref().map(|s| s.certificates.clone()).unwrap_or_default();
        if self.sso.certificates.read(cx).value().as_ref() != want {
            self.sso.certificates.update(cx, |s, cx| s.set_value(want, window, cx));
        }
        let placeholder = if p.email_domains.is_empty() { "acme.com" } else { "" };
        self.sso.domain.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        // The saved secret never comes back; an empty box keeps it.
        let kept = p.oidc.as_ref().is_some_and(|o| o.client_secret_set);
        if let Some(secret) = self.sso.line("client_secret").cloned() {
            let placeholder =
                if kept { t("instancesettings.provider.secretKept") } else { t("system.sso.clientSecret") };
            secret.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
    }

    /// Reads pasted metadata into the SAML fields.
    fn fill_metadata(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let xml = self.sso.metadata.read(cx).value().to_string();
        if xml.len() > sso::MAX_METADATA {
            self.sso.problem = Some(t("desktop.instance.metadataTooBig"));
            cx.notify();
            return;
        }
        let Some(found) = sso::read_saml_metadata(&xml).filter(|f| !f.entity_id.is_empty() || !f.sso_url.is_empty())
        else {
            self.sso.problem = Some(t("instancesettings.provider.notMetadata"));
            cx.notify();
            return;
        };
        self.patch_provider(cx, |p| {
            let s = p.saml.get_or_insert_with(Default::default);
            if !found.entity_id.is_empty() {
                s.entity_id = found.entity_id.clone();
            }
            if !found.sso_url.is_empty() {
                s.sso_url = found.sso_url.clone();
            }
            if !found.certificates.is_empty() {
                s.certificates = found.certificates.clone();
            }
        });
        self.sso.problem = found.sso_url.is_empty().then(|| t("instancesettings.provider.noRedirect"));
        self.sso.pasting = false;
        self.sso.filled += 1;
        self.sso.metadata.update(cx, |s, cx| s.set_value("", window, cx));
        self.sync_sso(window, cx);
        cx.notify();
    }

    /// Signs in through the saved provider in the browser, to see who it says you are.
    fn test_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.sso.tested, Some(Tested::Waiting)) {
            return;
        }
        self.sso.tested = Some(Tested::Waiting);
        let (core, key) = (self.core.clone(), self.key.clone());
        self.run(
            window,
            cx,
            async move { core.test_instance_sso(&key, crate::ui::open_in_browser).await },
            |this, result, _, cx| {
                this.sso.tested = Some(match result {
                    Ok(identity) => Tested::Signed(identity),
                    Err(problem) => Tested::Failed(problem.message),
                });
                cx.notify();
            },
        );
        cx.notify();
    }

    pub(super) fn sso_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let saved = self.saved().cloned().unwrap_or_default();
        let defaults = config.defaults.clone().unwrap_or_default();
        let provider = sso::full_provider(draft.sso_provider.as_ref());
        let ready = admin::provider_ready(Some(&provider));
        let (local_off, sso_off) = (pb::LocalAccounts::Off as i32, pb::SsoAccounts::Off as i32);
        let accounts =
            if draft.sso_accounts == pb::SsoAccounts::Unspecified as i32 { sso_off } else { draft.sso_accounts };
        let mut page = div().flex().flex_col();

        let not_ready = || t("instancesettings.sso.setUpFirst");
        let choice = self.choice(
            "sso",
            accounts,
            vec![
                Opt::new(
                    pb::SsoAccounts::Open as i32,
                    t("instancesettings.signUps.open"),
                    t("instancesettings.sso.openHint"),
                    "building",
                )
                .unless(!ready, not_ready),
                Opt::new(
                    pb::SsoAccounts::Closed as i32,
                    t("instancesettings.signUps.closed"),
                    t("instancesettings.sso.closedHint"),
                    "door-closed",
                )
                .unless(!ready, not_ready),
                Opt::new(sso_off, t("serversettings.shared.off"), t("instancesettings.sso.offHint"), "lock").unless(
                    !(accounts == sso_off || draft.local_accounts != local_off || admin::linked_works(&draft)),
                    || t("instancesettings.sso.offNeeds"),
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.sso_accounts = v),
        );
        let no_https = accounts != sso_off && !admin::can_return_to(&draft.public_url);
        page = page.child(
            self.setting(
                "sso-accounts",
                &t("instancesettings.nav.ssoAccounts"),
                Some(&t("instancesettings.sso.accountsHint")),
                &["sso_accounts"],
                &accounts_label(defaults.sso_accounts),
                0,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(choice)
                    .when(no_https, |el| el.child(self.notice("sso-https", &t("instancesettings.sso.notice"), p))),
                p,
                cx,
            ),
        );

        let protocol = provider.protocol;
        let picker = self.choice(
            "sso-protocol",
            protocol,
            vec![
                Opt::new(
                    pb::SsoProtocol::Unspecified as i32,
                    t("serversettings.shared.off"),
                    t("instancesettings.sso.noProvider"),
                    "power-off",
                ),
                // The protocols' own names.
                Opt::new(
                    pb::SsoProtocol::Oidc as i32,
                    "OpenID Connect".into(),
                    t("instancesettings.provider.oidcHint"),
                    "key-round",
                ),
                Opt::new(
                    pb::SsoProtocol::Saml as i32,
                    "SAML 2.0".into(),
                    t("instancesettings.provider.samlHint"),
                    "file-badge",
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch_provider(cx, |p| p.protocol = v),
        );
        page = page.child(self.setting(
            "sso-protocol",
            &t("serversettings.nav.ssoProtocol"),
            Some(&t("instancesettings.provider.protocolHint")),
            &["sso_provider"],
            &t("instancesettings.shared.none"),
            1,
            picker,
            p,
            cx,
        ));
        if protocol == pb::SsoProtocol::Unspecified as i32 {
            return page.into_any_element();
        }

        // The rest comes in fresh with each protocol.
        let mut form = div().flex().flex_col();
        if let Some(name) = self.sso.line("name") {
            let shown = provider.name.trim().to_owned();
            form = form.child(self.setting(
                "sso-name",
                &t("instancesettings.nav.name"),
                Some(&t("instancesettings.provider.nameHint")),
                &[],
                "",
                2,
                div().flex().flex_col().gap(px(10.0)).child(Input::new(name)).child(button_preview(&shown, p)),
                p,
                cx,
            ));
        }
        form = if protocol == pb::SsoProtocol::Oidc as i32 {
            self.oidc_fields(form, p, cx)
        } else {
            self.saml_fields(form, p, cx)
        };
        form = form.child(self.setting(
            "sso-domains",
            &t("serversettings.nav.ssoDomains"),
            Some(&t("instancesettings.provider.domainsHint")),
            &[],
            "",
            6,
            self.domains(&provider.email_domains, p, cx),
            p,
            cx,
        ));
        form = form.child(self.tell_provider(protocol, config.sso_service_provider.as_ref(), p, cx));
        let blocked = if admin::changed(&draft, &saved).iter().any(|c| c == "sso_provider") {
            Some(t("instancesettings.sso.saveFirst"))
        } else if !admin::provider_ready(saved.sso_provider.as_ref()) {
            Some(t("instancesettings.sso.fillFirst"))
        } else {
            None
        };
        form = form.child(self.test_row(blocked, p, window, cx));
        page.child(motion::rise(form, SharedString::from(format!("sso-form-{protocol}")), Duration::ZERO, 16.0))
            .into_any_element()
    }

    fn oidc_fields(&self, form: gpui_kit::Div, p: &Palette, cx: &mut Context<Self>) -> gpui_kit::Div {
        let mut form = form;
        if let Some(issuer) = self.sso.line("issuer") {
            form = form.child(self.setting(
                "sso-issuer",
                &t("system.sso.issuer"),
                Some(&t("instancesettings.provider.issuerHint")),
                &[],
                "",
                3,
                Input::new(issuer).prefix(icon("globe-lock").size(px(15.0)).text_color(p.muted_foreground)),
                p,
                cx,
            ));
        }
        if let (Some(id), Some(secret), Some(scopes)) =
            (self.sso.line("client_id"), self.sso.line("client_secret"), self.sso.line("scopes"))
        {
            form = form.child(
                self.setting(
                    "sso-client",
                    &t("system.sso.client"),
                    Some(&t("instancesettings.provider.clientHint")),
                    &[],
                    "",
                    4,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(div().flex_1().child(Input::new(id)))
                                .child(div().flex_1().child(Input::new(secret))),
                        )
                        .child(Input::new(scopes)),
                    p,
                    cx,
                ),
            );
        }
        form
    }

    fn saml_fields(&self, form: gpui_kit::Div, p: &Palette, cx: &mut Context<Self>) -> gpui_kit::Div {
        let s = &self.sso;
        let metadata = if s.pasting {
            let empty = s.metadata_empty;
            motion::rise(
                div().flex().flex_col().gap(px(8.0)).child(Textarea::new(&s.metadata)).child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            primary_button("sso-fill", t("instancesettings.provider.fillIn"), p)
                                .when(empty, |el| el.opacity(0.5))
                                .when(!empty, |el| {
                                    el.on_click(cx.listener(|this, _, window, cx| this.fill_metadata(window, cx)))
                                }),
                        )
                        .child(soft_button("sso-fill-cancel", t("common.cancel"), p).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.sso.pasting = false;
                                cx.notify();
                            },
                        ))),
                ),
                "sso-paste",
                Duration::ZERO,
                -8.0,
            )
            .into_any_element()
        } else {
            let filled = s.filled > 0 && s.problem.is_none();
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    soft_button("sso-paste-open", t("instancesettings.provider.pasteMetadata"), p)
                        .child(icon("clipboard-paste").size(px(15.0)))
                        .flex_row_reverse()
                        .gap(px(6.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.sso.pasting = true;
                            this.sso.problem = None;
                            this.sso.metadata.update(cx, |s, cx| s.focus(window, cx));
                            cx.notify();
                        })),
                )
                .when(filled, |el| {
                    el.child(motion::once(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(green())
                            .child(icon("check").size(px(14.0)))
                            .child(t("instancesettings.provider.filledIn")),
                        SharedString::from(format!("sso-filled-{}", s.filled)),
                        Duration::from_millis(360),
                        |el, t| {
                            let k = 1.0 - (1.0 - t).powi(3);
                            el.opacity(k).relative().top(px(4.0 * (1.0 - k)))
                        },
                    ))
                })
                .into_any_element()
        };
        let problem = s.problem.clone();
        let mut form = form.child(self.setting(
            "sso-metadata",
            &t("instancesettings.provider.metadata"),
            Some(&t("instancesettings.provider.metadataHint")),
            &[],
            "",
            3,
            div().flex().flex_col().gap(px(8.0)).child(metadata).when_some(problem, |el, problem| {
                el.child(div().text_xs().text_color(crate::ui::server_settings::amber(p)).child(problem))
            }),
            p,
            cx,
        ));
        if let Some(entity) = s.line("entity_id") {
            form = form.child(
                self.setting(
                    "sso-entity",
                    &t("instancesettings.provider.entity"),
                    Some(&t("instancesettings.provider.entityHint")),
                    &[],
                    "",
                    4,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(glow(Input::new(entity), "entity", s.filled, 0, p))
                        .child(glow(Input::new(&s.sso_url), "url", s.filled, 60, p)),
                    p,
                    cx,
                ),
            );
        }
        form.child(self.setting(
            "sso-certificates",
            &t("instancesettings.provider.certificates"),
            Some(&t("instancesettings.provider.certificatesHint")),
            &[],
            "",
            5,
            glow(Textarea::new(&s.certificates), "certs", s.filled, 120, p),
            p,
            cx,
        ))
    }

    /// Domains as chips, each taken off by a click; type one and press Enter (or a comma) to add it.
    fn domains(&self, domains: &[String], p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .p(px(6.0))
            .rounded(corner(12.0))
            .border_1()
            .border_color(p.border);
        for d in domains {
            let gone = d.clone();
            let hover = alpha(p.primary, 0.25);
            row = row.child(motion::once(
                div()
                    .id(SharedString::from(format!("sso-domain-{d}")))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .pl(px(10.0))
                    .pr(px(6.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(alpha(p.primary, 0.15))
                    .text_color(p.primary)
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .active(|s| s.top(px(1.0)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.patch_provider(cx, |p| p.email_domains.retain(|x| x != &gone));
                        this.sync_sso(window, cx);
                    }))
                    .child(format!("@{d}"))
                    .child(icon("x").size(px(12.0))),
                SharedString::from(format!("sso-domain-in-{d}")),
                Duration::from_millis(280),
                |el, t| {
                    let k = 1.0 - (1.0 - t).powi(3);
                    el.opacity(k)
                },
            ));
        }
        if domains.len() < MAX_DOMAINS {
            row = row.child(div().flex_1().min_w(px(160.0)).child(Input::new(&self.sso.domain).appearance(false)));
        }
        row.into_any_element()
    }

    /// What to enter at the provider, each with a copy button.
    fn tell_provider(
        &self,
        protocol: i32,
        sp: Option<&pb::ServiceProvider>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(sp) = sp else { return div().into_any_element() };
        let oidc = protocol == pb::SsoProtocol::Oidc as i32;
        // The protocols' own names for these, as the web shows them, but for the entity ID's.
        let rows: Vec<(String, String)> = if oidc {
            vec![("Redirect URI".to_owned(), sp.oidc_redirect_uri.clone())]
        } else {
            vec![
                (t("instancesettings.provider.entityAndMetadata"), sp.saml_entity_id.clone()),
                ("Assertion Consumer Service (HTTP-POST)".to_owned(), sp.saml_acs_url.clone()),
            ]
        };
        let hide = self.core.prefs().streamer_mode;
        let mut list = div().mt(px(12.0)).flex().flex_col().gap(px(8.0));
        for (n, (label, value)) in rows.into_iter().enumerate() {
            let copied = self.sso.copied == Some(n);
            let shown = if hide { hidden_address() } else { value.clone() };
            let row = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.muted_foreground)
                        .child(label.to_uppercase()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .pl(px(12.0))
                        .pr(px(4.0))
                        .py(px(4.0))
                        .rounded(corner(12.0))
                        .bg(alpha(p.muted, 0.6))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_family("monospace")
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(shown),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("sso-copy-{n}")))
                                .size(px(32.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(corner(8.0))
                                .cursor_pointer()
                                .text_color(if copied { green() } else { p.muted_foreground.into() })
                                .hover({
                                    let bg = p.background;
                                    move |s| s.bg(bg)
                                })
                                .active(|s| s.top(px(1.0)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                                    this.sso.copied = Some(n);
                                    cx.notify();
                                    cx.spawn_in(window, async move |this, cx| {
                                        cx.background_executor().timer(Duration::from_millis(1400)).await;
                                        let _ = this.update(cx, |this, cx| {
                                            if this.sso.copied == Some(n) {
                                                this.sso.copied = None;
                                                cx.notify();
                                            }
                                        });
                                    })
                                    .detach();
                                }))
                                .child(motion::once(
                                    div().child(icon(if copied { "check" } else { "copy" }).size(px(15.0))),
                                    SharedString::from(format!("sso-copy-{n}-{copied}")),
                                    Duration::from_millis(300),
                                    |el, t| {
                                        let k = 1.0 - (1.0 - t).powi(3);
                                        el.opacity(k)
                                    },
                                )),
                        ),
                );
            list = list.child(motion::slide_in(row, SharedString::from(format!("sso-tell-{protocol}-{n}")), -8.0));
        }
        motion::rise(
            div()
                .my(px(16.0))
                .p(px(16.0))
                .rounded(corner(16.0))
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(t("instancesettings.provider.tell")))
                .child(div().mt(px(2.0)).text_xs().text_color(p.muted_foreground).child(if oidc {
                    t("instancesettings.provider.tellOidc")
                } else {
                    t("instancesettings.provider.tellSaml")
                }))
                .child(list),
            SharedString::from(format!("sso-tell-{protocol}")),
            Duration::from_millis(150),
            12.0,
        )
        .into_any_element()
    }

    /// Signs in through the saved provider, and says who it signed in.
    fn test_row(
        &self,
        blocked: Option<String>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let waiting = matches!(self.sso.tested, Some(Tested::Waiting));
        let button = soft_button(
            "sso-test",
            if waiting { t("desktop.instance.waitingBrowser") } else { t("instancesettings.nav.ssoTest") },
            p,
        )
        .flex_row_reverse()
        .gap(px(6.0))
        .child(if waiting {
            spinner("sso-test-spin", 15.0, window).into_any_element()
        } else {
            icon("flask-conical").size(px(15.0)).into_any_element()
        })
        .when(blocked.is_some() || waiting, |el| el.opacity(0.6))
        .when(blocked.is_none() && !waiting, |el| {
            el.on_click(cx.listener(|this, _, window, cx| this.test_sign_in(window, cx)))
        });
        let result: Option<AnyElement> = match &self.sso.tested {
            Some(Tested::Failed(message)) => {
                Some(div().text_sm().text_color(p.destructive).child(message.clone()).into_any_element())
            }
            Some(Tested::Signed(identity)) => Some(self.identity(identity.as_ref(), p)),
            _ => None,
        };
        self.setting(
            "sso-test",
            &t("instancesettings.nav.ssoTest"),
            Some(&t("desktop.instance.ssoTestHint")),
            &[],
            "",
            7,
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div().flex().items_center().gap(px(12.0)).child(button).when_some(blocked, |el, why| {
                        el.child(div().text_xs().text_color(p.muted_foreground).child(why))
                    }),
                )
                .when_some(result, |el, r| el.child(r)),
            p,
            cx,
        )
    }

    /// Who the provider said signed in, after a test.
    fn identity(&self, identity: Option<&pb::SsoIdentity>, p: &Palette) -> AnyElement {
        let hide = self.core.prefs().streamer_mode;
        let id = identity.cloned().unwrap_or_default();
        let rows = [
            (t("connect.callback.identity.name"), id.name.clone()),
            (
                t("connect.callback.identity.email"),
                if hide && !id.email.is_empty() { t("desktop.instance.ssoHidden") } else { id.email.clone() },
            ),
            (t("desktop.instance.ssoSubject"), id.subject.clone()),
        ];
        let mut list = div().flex().flex_col().gap(px(4.0));
        for (label, value) in rows {
            list = list.child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .text_sm()
                    .child(div().w(px(70.0)).flex_none().text_color(p.muted_foreground).child(label))
                    .child(div().min_w_0().text_ellipsis().font_weight(FontWeight::BOLD).child(if value.is_empty() {
                        "—".to_owned()
                    } else {
                        value
                    })),
            );
        }
        motion::rise(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(12.0))
                .rounded(corner(14.0))
                .border_1()
                .border_color(green().opacity(0.4))
                .bg(green().opacity(0.08))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_sm()
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(green())
                        .child(icon("circle-check").size(px(16.0)))
                        .child(t("desktop.instance.ssoWorks")),
                )
                .child(list),
            SharedString::from(format!("sso-identity-{}", id.subject)),
            Duration::ZERO,
            8.0,
        )
        .into_any_element()
    }
}

fn green() -> gpui_kit::Hsla {
    hsla(0.42, 0.65, 0.45, 1.0)
}

/// A live "Continue with …" as people will see it.
fn button_preview(name: &str, p: &Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .text_xs()
        .text_color(p.muted_foreground)
        .child(t("instancesettings.provider.showsAs"))
        .child(
            div()
                .h(px(36.0))
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .rounded(corner(8.0))
                .bg(p.foreground)
                .text_color(p.background)
                .text_sm()
                .font_weight(FontWeight::EXTRA_BOLD)
                .child(icon("building").size(px(16.0)).text_color(p.primary))
                .child(motion::rise(
                    div().text_ellipsis().whitespace_nowrap().child(t_with(
                        "connect.provider.continueWith",
                        &[("name", Arg::Str(if name.is_empty() { "…" } else { name }))],
                    )),
                    SharedString::from(format!("sso-preview-{name}")),
                    Duration::ZERO,
                    6.0,
                )),
        )
        .into_any_element()
}

/// Lights a field up for a moment after metadata filled it: a ring around
/// it that comes and goes, without moving it.
fn glow(child: impl IntoElement, id: &str, filled: u32, delay_ms: u64, p: &Palette) -> AnyElement {
    let ring = p.primary;
    let total = 1000 + delay_ms;
    let start = delay_ms as f32 / total as f32;
    div()
        .relative()
        .child(child)
        .when(filled > 0, |el| {
            el.child(motion::once(
                div().absolute().top(px(-3.0)).bottom(px(-3.0)).left(px(-3.0)).right(px(-3.0)).rounded(corner(14.0)),
                SharedString::from(format!("sso-glow-{id}-{filled}")),
                Duration::from_millis(total),
                move |el, t| {
                    let k = ((t - start) / (1.0 - start)).clamp(0.0, 1.0);
                    el.border_3().border_color(alpha(ring, 0.5 * (k * std::f32::consts::PI).sin()))
                },
            ))
        })
        .into_any_element()
}
