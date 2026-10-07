//! Single sign-on for one server, the web's `settings/server/SingleSignOn.tsx`
//! over `settings/IdentityProviderForm.tsx`: the identity provider members
//! sign in through (OpenID Connect or SAML, its details, which email domains
//! get in, what to tell the provider), your own sign-in through it, whether
//! it's required to join and to stay, and how often members sign in again.
//! Only the owner sees it.

use gpui_kit::ClipboardItem;

use super::pages::{boxed, focused, setting, shimmers};
use super::*;
use crate::core::server_pages::{RECHECKS, provider_print, provider_ready};
use crate::core::sso::{MAX_DOMAINS, add_domains, full_provider, read_saml_metadata};
use crate::ui::settings_controls::{Look, Opt, button, choice, toggle};
use crate::ui::theme::{radius_2xl, radius_lg, radius_xl};

/// The provider's boxes, in the order the form shows them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Name,
    Issuer,
    ClientId,
    Secret,
    Scopes,
    Entity,
    SsoUrl,
}

const LINES: [Line; 7] =
    [Line::Name, Line::Issuer, Line::ClientId, Line::Secret, Line::Scopes, Line::Entity, Line::SsoUrl];

impl Line {
    fn get(self, p: &pb::IdentityProvider) -> String {
        let o = p.oidc.clone().unwrap_or_default();
        let s = p.saml.clone().unwrap_or_default();
        match self {
            Line::Name => p.name.clone(),
            Line::Issuer => o.issuer,
            Line::ClientId => o.client_id,
            Line::Secret => o.client_secret,
            Line::Scopes => o.extra_scopes,
            Line::Entity => s.entity_id,
            Line::SsoUrl => s.sso_url,
        }
    }

    fn set(self, p: &mut pb::IdentityProvider, v: String) {
        match self {
            Line::Name => p.name = v.chars().take(40).collect(),
            Line::Issuer => p.oidc.get_or_insert_with(Default::default).issuer = v,
            Line::ClientId => p.oidc.get_or_insert_with(Default::default).client_id = v,
            Line::Secret => p.oidc.get_or_insert_with(Default::default).client_secret = v,
            Line::Scopes => p.oidc.get_or_insert_with(Default::default).extra_scopes = v,
            Line::Entity => p.saml.get_or_insert_with(Default::default).entity_id = v,
            Line::SsoUrl => p.saml.get_or_insert_with(Default::default).sso_url = v,
        }
    }

    fn placeholder(self) -> String {
        match self {
            Line::Name => "Acme".into(),
            Line::Issuer => "https://login.acme.com".into(),
            Line::ClientId => t("system.sso.clientId"),
            Line::Secret => t("system.sso.clientSecret"),
            Line::Scopes => t("instancesettings.provider.scopes"),
            Line::Entity => "https://idp.acme.com/metadata".into(),
            Line::SsoUrl => "https://idp.acme.com/sso/redirect".into(),
        }
    }
}

pub(super) struct Sso {
    saved: Option<pb::ServerSso>,
    mine: Option<pb::SsoIdentity>,
    loading: bool,
    load_error: Option<String>,
    provider: pb::IdentityProvider,
    required: bool,
    recheck: i32,
    lines: Vec<Entity<InputState>>,
    certificates: Entity<TextareaState>,
    metadata: Entity<TextareaState>,
    domain: Entity<InputState>,
    pasting: bool,
    filled: u32,
    metadata_problem: Option<String>,
    saving: bool,
    save_error: Option<String>,
    signing_in: bool,
    sign_in_error: Option<String>,
    copied: Option<(usize, Instant)>,
    /// The boxes are being filled from the provider, so their changes aren't edits.
    filling: bool,
}

impl Sso {
    pub(super) fn new(window: &mut Window, cx: &mut Context<ServerSettingsView>) -> (Self, Vec<Subscription>) {
        let mut subscriptions = Vec::new();
        let mut lines = Vec::new();
        for (n, line) in LINES.iter().enumerate() {
            let masked = *line == Line::Secret;
            let state = cx.new(|cx| {
                let s = InputState::new(window, cx).placeholder(line.placeholder());
                if masked { s.masked(true) } else { s }
            });
            subscriptions.push(cx.subscribe(&state, move |this: &mut ServerSettingsView, s, e: &InputEvent, cx| {
                if let InputEvent::Change = e {
                    if !this.pages.sso.filling {
                        let v = s.read(cx).value().to_string();
                        LINES[n].set(&mut this.pages.sso.provider, v);
                    }
                    cx.notify();
                }
            }));
            lines.push(state);
        }
        let certificates = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 10)
                .placeholder("-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----")
        });
        subscriptions.push(cx.subscribe(&certificates, |this: &mut ServerSettingsView, s, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                if !this.pages.sso.filling {
                    let v = s.read(cx).value().to_string();
                    this.pages.sso.provider.saml.get_or_insert_with(Default::default).certificates = v;
                }
                cx.notify();
            }
        }));
        let metadata = cx.new(|cx| TextareaState::new(window, cx).auto_grow(5, 10).placeholder("<EntityDescriptor …>"));
        subscriptions.push(cx.subscribe(&metadata, |_: &mut ServerSettingsView, _, e: &InputEvent, cx| {
            if let InputEvent::Change = e {
                cx.notify();
            }
        }));
        let domain = cx.new(|cx| InputState::new(window, cx).placeholder("acme.com"));
        subscriptions.push(cx.subscribe_in(
            &domain,
            window,
            |this: &mut ServerSettingsView, s, e: &InputEvent, window, cx| match e {
                InputEvent::PressEnter { .. } => this.add_domain(window, cx),
                InputEvent::Change => {
                    if s.read(cx).value().contains(',') {
                        this.add_domain(window, cx);
                    }
                    cx.notify();
                }
                InputEvent::Blur => this.add_domain(window, cx),
                _ => {}
            },
        ));
        let sso = Self {
            saved: None,
            mine: None,
            loading: false,
            load_error: None,
            provider: full_provider(None),
            required: false,
            recheck: 30,
            lines,
            certificates,
            metadata,
            domain,
            pasting: false,
            filled: 0,
            metadata_problem: None,
            saving: false,
            save_error: None,
            signing_in: false,
            sign_in_error: None,
            copied: None,
            filling: false,
        };
        (sso, subscriptions)
    }
}

impl ServerSettingsView {
    fn add_domain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.pages.sso;
        let text = s.domain.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        add_domains(&mut s.provider.email_domains, &text);
        s.domain.update(cx, |d, cx| d.set_value("", window, cx));
        cx.notify();
    }

    /// Shows `sso` as saved, and its provider in the boxes.
    fn take_sso(&mut self, sso: pb::ServerSso, window: &mut Window, cx: &mut Context<Self>) {
        let provider = full_provider(sso.provider.as_ref());
        self.fill_provider(provider, window, cx);
        let s = &mut self.pages.sso;
        s.required = sso.required;
        s.recheck = sso.recheck_days;
        s.saved = Some(sso);
    }

    fn fill_provider(&mut self, provider: pb::IdentityProvider, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.sso.filling = true;
        for (n, line) in LINES.iter().enumerate() {
            let v = line.get(&provider);
            self.pages.sso.lines[n].update(cx, |s, cx| s.set_value(v, window, cx));
        }
        let certs = provider.saml.as_ref().map(|s| s.certificates.clone()).unwrap_or_default();
        self.pages.sso.certificates.update(cx, |s, cx| s.set_value(certs, window, cx));
        self.pages.sso.filling = false;
        self.pages.sso.provider = provider;
    }

    fn load_sso(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &mut self.pages.sso;
        if s.loading || s.saved.is_some() || s.load_error.is_some() {
            return;
        }
        s.loading = true;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = self.core.spawn(async move { core.get_server_sso(&key, &sid).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pages.sso.loading = false;
                match result {
                    Ok((sso, mine)) => {
                        this.take_sso(sso, window, cx);
                        this.pages.sso.mine = mine;
                    }
                    Err(err) => this.pages.sso.load_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_sso(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &self.pages.sso;
        let Some(before) = s.saved.clone() else { return };
        let provider_dirty = provider_print(Some(&s.provider)) != provider_print(before.provider.as_ref());
        let removing = s.provider.protocol == pb::SsoProtocol::Unspecified as i32;
        let req = pb::UpdateServerSsoRequest {
            server_id: self.server.clone(),
            provider: (provider_dirty && !removing).then(|| s.provider.clone()),
            remove_provider: provider_dirty && removing,
            // Only what changed: a new provider stops requiring itself unless asked again.
            required: (!removing && s.required != before.required).then_some(s.required),
            recheck_days: (s.recheck != before.recheck_days).then_some(s.recheck),
        };
        self.pages.sso.saving = true;
        self.pages.sso.save_error = None;
        let (core, key) = (self.core.clone(), self.key.clone());
        let rx = self.core.spawn(async move { core.update_server_sso(&key, req).await });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pages.sso.saving = false;
                match result {
                    Ok(sso) => {
                        this.take_sso(sso, window, cx);
                        if provider_dirty {
                            this.pages.sso.mine = None;
                        }
                    }
                    Err(err) => this.pages.sso.save_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Signs in through the saved provider in the browser, then reads who signed in.
    fn sso_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pages.sso.signing_in = true;
        self.pages.sso.sign_in_error = None;
        let (core, key, sid) = (self.core.clone(), self.key.clone(), self.server.clone());
        let rx = self.core.spawn(async move {
            core.server_sso(&key, &sid, None, crate::ui::open_in_browser).await?;
            core.get_server_sso(&key, &sid).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |this, _, cx| {
                let s = &mut this.pages.sso;
                s.signing_in = false;
                match result {
                    Ok((sso, mine)) => {
                        s.mine = mine;
                        if let Some(saved) = &mut s.saved {
                            saved.signed_in_members = sso.signed_in_members;
                        }
                    }
                    Err(err) => s.sign_in_error = Some(err.message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn fill_from_metadata(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let xml = self.pages.sso.metadata.read(cx).value().to_string();
        let found = read_saml_metadata(&xml).filter(|f| !f.entity_id.is_empty() || !f.sso_url.is_empty());
        let Some(found) = found else {
            self.pages.sso.metadata_problem = Some(t("instancesettings.provider.notMetadata"));
            cx.notify();
            return;
        };
        let mut provider = self.pages.sso.provider.clone();
        let saml = provider.saml.get_or_insert_with(Default::default);
        if !found.entity_id.is_empty() {
            saml.entity_id = found.entity_id.clone();
        }
        if !found.sso_url.is_empty() {
            saml.sso_url = found.sso_url.clone();
        }
        if !found.certificates.is_empty() {
            saml.certificates = found.certificates.clone();
        }
        self.fill_provider(provider, window, cx);
        let s = &mut self.pages.sso;
        s.metadata_problem = found.sso_url.is_empty().then(|| t("instancesettings.provider.noRedirect"));
        s.pasting = false;
        s.filled += 1;
        s.metadata.update(cx, |m, cx| m.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn sso_page(
        &mut self,
        server: &pb::Server,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.load_sso(window, cx);
        if let Some(e) = &self.pages.sso.load_error {
            return super::pages::problem(e, p);
        }
        let Some(saved) = self.pages.sso.saved.clone() else {
            return shimmers(3, 112.0, radius_2xl(), p, window);
        };
        let owner = self.core.shared.read(|s| s.instance(&self.key).is_some_and(|i| i.access(&self.server).owner));
        let s = &self.pages.sso;
        let provider = s.provider.clone();
        let provider_dirty = provider_print(Some(&provider)) != provider_print(saved.provider.as_ref());
        let changes = [provider_dirty, s.required != saved.required, s.recheck != saved.recheck_days]
            .into_iter()
            .filter(|c| *c)
            .count();
        let saved_ready = provider_ready(saved.provider.as_ref());
        let name = saved
            .provider
            .as_ref()
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| t("serversettings.sso.theProvider"));
        let may_require = saved.required || owner || s.mine.is_some();
        if changes > 0 {
            self.bar = Some(bar_with_error(
                "sso-bar",
                changes,
                s.saving,
                s.save_error.as_deref(),
                p,
                cx,
                |this, window, cx| {
                    if let Some(saved) = this.pages.sso.saved.clone() {
                        this.take_sso(saved, window, cx);
                    }
                    this.pages.sso.save_error = None;
                    cx.notify();
                },
                |this, window, cx| this.save_sso(window, cx),
            ));
        }
        let on = provider.protocol != pb::SsoProtocol::Unspecified as i32;
        let protocol = pb::SsoProtocol::try_from(provider.protocol).unwrap_or(pb::SsoProtocol::Unspecified);
        let mut page = div().flex().flex_col().child(self.sso_summary(&saved, server, &name, p));

        // The provider (the web's `IdentityProviderForm`).
        let protocol_setting = setting(
            &t("serversettings.nav.ssoProtocol"),
            Some(&t("instancesettings.provider.protocolHint")),
            true,
            !on,
            p,
        )
        .child(choice(
            "sso-protocol",
            Some(match protocol {
                pb::SsoProtocol::Unspecified => 0,
                pb::SsoProtocol::Oidc => 1,
                pb::SsoProtocol::Saml => 2,
            }),
            vec![
                Opt::new(t("serversettings.shared.off"), t("serversettings.sso.offHint"), "power-off"),
                Opt::new("OpenID Connect", t("instancesettings.provider.oidcHint"), "key-round"),
                Opt::new("SAML 2.0", t("instancesettings.provider.samlHint"), "file-badge"),
            ],
            self.column,
            p,
            window,
            cx,
            |this: &mut Self, i, cx| {
                this.pages.sso.provider.protocol = match i {
                    1 => pb::SsoProtocol::Oidc,
                    2 => pb::SsoProtocol::Saml,
                    _ => pb::SsoProtocol::Unspecified,
                } as i32;
                cx.notify();
            },
        ));
        page = page.child(self.mark("sso-protocol", protocol_setting, p));
        if on {
            let mut fields = div().flex().flex_col();
            let line = |this: &Self, n: usize, window: &Window, cx: &gpui_kit::App| {
                let state = this.pages.sso.lines[n].clone();
                let on = focused(&state, window, cx);
                boxed(Input::new(&state).appearance(false), 40.0, on, p)
            };
            let name_now = provider.name.trim().to_owned();
            fields = fields.child(
                setting(
                    &t("instancesettings.nav.name"),
                    Some(&t("instancesettings.provider.nameHint")),
                    false,
                    false,
                    p,
                )
                .child(line(self, 0, window, cx))
                .child(button_preview(&name_now, p)),
            );
            if protocol == pb::SsoProtocol::Oidc {
                let issuer = self.pages.sso.lines[1].clone();
                fields = fields
                    .child(
                        setting(
                            &t("system.sso.issuer"),
                            Some(&t("instancesettings.provider.issuerHint")),
                            false,
                            false,
                            p,
                        )
                        .child(
                            div()
                                .relative()
                                .child(
                                    boxed(Input::new(&issuer).appearance(false), 40.0, focused(&issuer, window, cx), p)
                                        .pl(px(28.0)),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(12.0))
                                        .top(px(12.0))
                                        .text_color(p.muted_foreground)
                                        .child(icon("globe-lock").size(px(16.0))),
                                ),
                        ),
                    )
                    .child(
                        setting(
                            &t("system.sso.client"),
                            Some(&t("instancesettings.provider.clientHint")),
                            false,
                            false,
                            p,
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(div().flex_1().child(line(self, 2, window, cx)))
                                .child(div().flex_1().child(line(self, 3, window, cx))),
                        )
                        .child(line(self, 4, window, cx)),
                    );
            } else {
                fields = fields
                    .child(self.metadata_setting(p, window, cx))
                    .child(
                        setting(
                            &t("instancesettings.provider.entity"),
                            Some(&t("instancesettings.provider.entityHint")),
                            false,
                            false,
                            p,
                        )
                        .child(glow(line(self, 5, window, cx), self.pages.sso.filled, 0, p))
                        .child(glow(line(self, 6, window, cx), self.pages.sso.filled, 60, p)),
                    )
                    .child({
                        let certs = self.pages.sso.certificates.clone();
                        setting(
                            &t("instancesettings.provider.certificates"),
                            Some(&t("instancesettings.provider.certificatesHint")),
                            false,
                            false,
                            p,
                        )
                        .child(glow(
                            crate::ui::instance_home::focus_ring(
                                div()
                                    .w_full()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(radius_xl())
                                    .border_1()
                                    .border_color(p.border)
                                    .text_xs()
                                    .font_family("monospace")
                                    .child(Textarea::new(&certs).appearance(false)),
                                focused(&certs, window, cx),
                                p,
                            ),
                            self.pages.sso.filled,
                            120,
                            p,
                        ))
                    });
            }
            let domains = self.domains(&provider.email_domains, p, window, cx);
            fields = fields.child(
                self.mark(
                    "sso-domains",
                    setting(
                        &t("serversettings.nav.ssoDomains"),
                        Some(&t("instancesettings.provider.domainsHint")),
                        false,
                        false,
                        p,
                    )
                    .child(domains),
                    p,
                ),
            );
            fields = fields.child(self.tell_provider(protocol, saved.service_provider.as_ref(), p, cx));
            page = page.child(motion::rise(
                fields,
                SharedString::from(format!("sso-fields-{}", provider.protocol)),
                Duration::ZERO,
                16.0,
            ));

            // Your sign-in, requiring it, and how often.
            let mine = self.pages.sso.mine.clone();
            let mut yours =
                setting(&t("serversettings.sso.yours"), Some(&t("serversettings.sso.yoursHint")), false, false, p);
            yours = match (&mine, provider_dirty) {
                (Some(identity), false) => yours.child(identity_card(identity, p)),
                _ => yours.child(div().text_sm().text_color(p.muted_foreground).child(if provider_dirty {
                    t("serversettings.sso.saveFirst")
                } else {
                    t("serversettings.sso.notYet")
                })),
            };
            if saved_ready && !provider_dirty {
                let label = if mine.is_some() {
                    t_with("serversettings.sso.signInAgain", &[("name", Arg::Str(&name))])
                } else {
                    t_with("join.sso.title", &[("name", Arg::Str(&name))])
                };
                let busy = self.pages.sso.signing_in;
                let (fg, bg) = (p.background, p.foreground);
                yours = yours
                    .child(
                        div()
                            .id("sso-server-sign-in")
                            .h(px(48.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(10.0))
                            .px(px(16.0))
                            .rounded(radius_xl())
                            .bg(bg)
                            .text_color(fg)
                            .font_weight(FontWeight::EXTRA_BOLD)
                            .shadow(vec![gpui_kit::BoxShadow {
                                color: p.primary.into(),
                                offset: gpui_kit::point(px(0.0), px(14.0)),
                                blur_radius: px(30.0),
                                spread_radius: px(-16.0),
                                inset: false,
                            }])
                            .cursor_pointer()
                            .hover(|s| s.top(px(-2.0)))
                            .active(|s| s.top(px(1.0)))
                            .when(!busy, |el| {
                                el.on_click(cx.listener(|this, _, window, cx| this.sso_sign_in(window, cx)))
                            })
                            .child(div().text_color(p.primary).child(if busy {
                                spinner("sso-sign-in-spin", 20.0, window)
                            } else {
                                icon("user-check").size(px(20.0)).into_any_element()
                            }))
                            .child(label),
                    )
                    .when_some(self.pages.sso.sign_in_error.clone(), |el, e| {
                        el.child(
                            div().text_sm().text_color(p.destructive).child(crate::ui::instance_home::capitalized(&e)),
                        )
                    });
            }
            page = page.child(yours);
            let locked = !saved_ready || provider_dirty;
            let required = self.pages.sso.required;
            page = page.child(self.mark(
                "sso-required",
                setting(&t("serversettings.sso.requireIt"), None, false, false, p).child(toggle(
                    "sso-required",
                    &t_with("serversettings.sso.requireLabel", &[("name", Arg::Str(&name))]),
                    Some(&if locked {
                        t("serversettings.sso.saveProviderFirst")
                    } else if !may_require {
                        t("serversettings.sso.signInYourselfFirst")
                    } else {
                        t("serversettings.sso.requireHint")
                    }),
                    required,
                    locked || !may_require,
                    p,
                    window,
                    cx,
                    |this: &mut Self, on, cx| {
                        this.pages.sso.required = on;
                        cx.notify();
                    },
                )),
                p,
            ));
            let recheck = if RECHECKS.contains(&self.pages.sso.recheck) { self.pages.sso.recheck } else { 30 };
            let options: Vec<Opt> = RECHECKS
                .iter()
                .map(|&days| {
                    let label = match days {
                        0 => t("serversettings.shared.never"),
                        7 => t("serversettings.sso.everyWeek"),
                        d => t_with("serversettings.sso.everyDays", &[("count", Arg::Num(i64::from(d)))]),
                    };
                    let hint = match days {
                        0 => t("serversettings.sso.onceEnough"),
                        7 => t("serversettings.sso.tightest"),
                        30 => t("serversettings.sso.goodDefault"),
                        _ => t("serversettings.sso.lightest"),
                    };
                    let glyph = match days {
                        0 => "infinity",
                        7 => "calendar-clock",
                        _ => "calendar-days",
                    };
                    Opt::new(label, hint, glyph)
                })
                .collect();
            page = page.child(
                self.mark(
                    "sso-recheck",
                    setting(
                        &t("serversettings.nav.ssoRecheck"),
                        Some(&t("serversettings.sso.recheckHint")),
                        false,
                        true,
                        p,
                    )
                    .child(choice(
                        "sso-recheck",
                        RECHECKS.iter().position(|d| *d == recheck),
                        options,
                        self.column,
                        p,
                        window,
                        cx,
                        |this: &mut Self, i, cx| {
                            this.pages.sso.recheck = RECHECKS[i];
                            cx.notify();
                        },
                    )),
                    p,
                ),
            );
        }
        page.into_any_element()
    }

    /// The headline: whether sign-on is set up and required, and how many members signed in.
    fn sso_summary(&self, saved: &pb::ServerSso, server: &pb::Server, name: &str, p: &Palette) -> AnyElement {
        let ready = provider_ready(saved.provider.as_ref());
        let title = if !ready {
            t("serversettings.sso.notSetUp")
        } else if saved.required {
            t_with("serversettings.sso.required", &[("name", Arg::Str(name))])
        } else {
            t_with("serversettings.sso.notRequired", &[("name", Arg::Str(name))])
        };
        let line = if ready {
            let text = t_with(
                "serversettings.sso.signedIn",
                &[
                    ("signedIn", Arg::Str(&strong(&saved.signed_in_members.to_string()))),
                    ("count", Arg::Num(server.member_count)),
                ],
            );
            div().child(marked(&text, p)).into_any_element()
        } else {
            div().child(t("serversettings.sso.usualWay")).into_any_element()
        };
        let share = (saved.signed_in_members as f32 / server.member_count.max(1) as f32).min(1.0);
        motion::rise(
            div()
                .relative()
                .mb(px(24.0))
                .overflow_hidden()
                .rounded(radius_2xl())
                .border_1()
                .border_color(p.border)
                .p(px(16.0))
                .bg(gpui_kit::linear_gradient(
                    135.0,
                    gpui_kit::linear_color_stop(alpha(p.primary, 0.1), 0.0),
                    gpui_kit::linear_color_stop(alpha(p.primary, 0.0), 0.5),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.0))
                        .child(
                            div()
                                .size(px(48.0))
                                .flex_none()
                                .rounded(radius_2xl())
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(alpha(p.primary, 0.15))
                                .text_color(p.primary)
                                .child(icon(if saved.required { "shield-check" } else { "building" }).size(px(24.0))),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(div().font_weight(FontWeight::EXTRA_BOLD).child(title))
                                .child(
                                    div().text_sm().line_height(px(20.0)).text_color(p.muted_foreground).child(line),
                                ),
                        ),
                )
                .when(ready, |el| {
                    el.child(div().mt(px(12.0)).h(px(6.0)).rounded_full().overflow_hidden().bg(p.muted).child(
                        motion::once(
                            div().h_full().rounded_full().bg(p.primary),
                            "sso-share",
                            Duration::from_millis(900),
                            move |el, t| {
                                let e = 1.0 - (1.0 - t).powi(3);
                                el.w(gpui_kit::relative(share * e))
                            },
                        ),
                    ))
                }),
            "sso-summary",
            Duration::ZERO,
            12.0,
        )
        .into_any_element()
    }

    /// SAML metadata pasted in fills the entity ID, sign-in URL and certificates.
    fn metadata_setting(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.pages.sso;
        let mut el = setting(
            &t("instancesettings.provider.metadata"),
            Some(&t("instancesettings.provider.metadataHint")),
            false,
            false,
            p,
        );
        if s.pasting {
            let xml = s.metadata.clone();
            let empty = xml.read(cx).value().trim().is_empty();
            el = el.child(super::pages::slide_in(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(crate::ui::instance_home::focus_ring(
                        div()
                            .w_full()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(radius_xl())
                            .border_1()
                            .border_color(p.border)
                            .text_xs()
                            .font_family("monospace")
                            .child(Textarea::new(&xml).appearance(false)),
                        focused(&xml, window, cx),
                        p,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                button(
                                    "sso-fill",
                                    t("instancesettings.provider.fillIn"),
                                    None,
                                    Look::Primary,
                                    false,
                                    p,
                                )
                                .rounded(radius_xl())
                                .font_weight(FontWeight::BOLD)
                                .when(empty, |el| el.opacity(0.5))
                                .when(!empty, |el| {
                                    el.on_click(cx.listener(|this, _, window, cx| this.fill_from_metadata(window, cx)))
                                }),
                            )
                            .child(
                                button("sso-fill-cancel", t("common.cancel"), None, Look::Ghost, false, p)
                                    .rounded(radius_xl())
                                    .font_weight(FontWeight::BOLD)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pages.sso.pasting = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
                "sso-paste",
            ));
        } else {
            let filled = s.filled > 0 && s.metadata_problem.is_none();
            el = el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        button(
                            "sso-paste-open",
                            t("instancesettings.provider.pasteMetadata"),
                            Some("clipboard-paste"),
                            Look::Outline,
                            false,
                            p,
                        )
                        .rounded(radius_xl())
                        .font_weight(FontWeight::BOLD)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.pages.sso.pasting = true;
                            let xml = this.pages.sso.metadata.clone();
                            xml.update(cx, |s, cx| s.focus(window, cx));
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
                                .text_color(gpui_kit::rgb(0x10b981))
                                .child(icon("check").size(px(14.0)))
                                .child(t("instancesettings.provider.filledIn")),
                            SharedString::from(format!("sso-filled-{}", s.filled)),
                            Duration::from_millis(300),
                            |el, t| el.opacity(t),
                        ))
                    }),
            );
        }
        if let Some(problem) = s.metadata_problem.clone() {
            el = el.child(div().text_xs().text_color(amber(p)).child(problem));
        }
        el.into_any_element()
    }

    /// Email domains as chips: type and press Enter (or a comma), click one to take it off.
    fn domains(&mut self, list: &[String], p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = self.pages.sso.domain.clone();
        let on = focused(&state, window, cx);
        let primary = p.primary;
        let mut row = div()
            .min_h(px(40.0))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .p(px(6.0))
            .rounded(radius_xl())
            .border_1()
            .border_color(p.border)
            .when(on, |el| {
                el.shadow(vec![gpui_kit::BoxShadow {
                    color: alpha(p.primary, 0.4),
                    offset: gpui_kit::point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(2.0),
                    inset: false,
                }])
            });
        for d in list.iter() {
            let gone = d.clone();
            row = row.child(motion::once(
                div()
                    .id(SharedString::from(format!("domain-{d}")))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .rounded_full()
                    .bg(alpha(primary, 0.15))
                    .py(px(4.0))
                    .pl(px(10.0))
                    .pr(px(6.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(primary)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pages.sso.provider.email_domains.retain(|x| *x != gone);
                        cx.notify();
                    }))
                    .child(format!("@{d}"))
                    .child(icon("x").size(px(12.0))),
                SharedString::from(format!("domain-in-{d}")),
                Duration::from_millis(240),
                |el, t| el.opacity(t),
            ));
        }
        if list.len() < MAX_DOMAINS {
            row = row.child(
                div()
                    .h(px(28.0))
                    .min_w(px(96.0))
                    .flex_1()
                    .px(px(6.0))
                    .text_sm()
                    .child(Input::new(&state).appearance(false)),
            );
        }
        row.into_any_element()
    }

    /// What to enter at the provider, each with a copy button.
    fn tell_provider(
        &mut self,
        protocol: pb::SsoProtocol,
        sp: Option<&pb::ServiceProvider>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(sp) = sp else { return div().into_any_element() };
        let rows: Vec<(String, String)> = if protocol == pb::SsoProtocol::Oidc {
            vec![("Redirect URI".into(), sp.oidc_redirect_uri.clone())]
        } else {
            vec![
                (t("instancesettings.provider.entityAndMetadata"), sp.saml_entity_id.clone()),
                ("Assertion Consumer Service (HTTP-POST)".into(), sp.saml_acs_url.clone()),
            ]
        };
        let copied = self.pages.sso.copied.filter(|(_, at)| at.elapsed() < Duration::from_millis(1400)).map(|(n, _)| n);
        let mut list = div().mt(px(12.0)).flex().flex_col().gap(px(8.0));
        for (n, (label, value)) in rows.into_iter().enumerate() {
            let done = copied == Some(n);
            let (hover_bg, hover_fg) = (p.background, p.foreground);
            let text = value.clone();
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.2))
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.muted_foreground)
                            .child(label.to_uppercase()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .rounded(radius_xl())
                            .bg(alpha(p.muted, 0.6))
                            .py(px(4.0))
                            .pr(px(4.0))
                            .pl(px(12.0))
                            .child(div().flex_1().min_w_0().truncate().text_xs().font_family("monospace").child(value))
                            .child(
                                div()
                                    .id(SharedString::from(format!("sso-copy-{n}")))
                                    .size(px(32.0))
                                    .flex_none()
                                    .rounded(radius_lg())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(if done { gpui_kit::rgb(0x10b981) } else { p.muted_foreground })
                                    .cursor_pointer()
                                    .when(!done, |el| el.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                        this.pages.sso.copied = Some((n, Instant::now()));
                                        cx.notify();
                                        cx.spawn(async move |this, cx| {
                                            cx.background_executor().timer(Duration::from_millis(1450)).await;
                                            let _ = this.update(cx, |_, cx| cx.notify());
                                        })
                                        .detach();
                                    }))
                                    .child(icon(if done { "check" } else { "copy" }).size(px(16.0))),
                            ),
                    ),
            );
        }
        motion::rise(
            div()
                .my(px(16.0))
                .p(px(16.0))
                .rounded(radius_2xl())
                .border_1()
                .border_dashed()
                .border_color(p.border)
                .child(div().text_sm().font_weight(FontWeight::EXTRA_BOLD).child(t("instancesettings.provider.tell")))
                .child(div().mt(px(2.0)).text_xs().line_height(px(16.0)).text_color(p.muted_foreground).child(t(
                    if protocol == pb::SsoProtocol::Oidc {
                        "instancesettings.provider.tellOidc"
                    } else {
                        "instancesettings.provider.tellSaml"
                    },
                )))
                .child(list),
            "sso-tell",
            Duration::from_millis(150),
            12.0,
        )
        .into_any_element()
    }
}

/// A live "Continue with ..." as people will see it.
fn button_preview(name: &str, p: &Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .text_xs()
        .text_color(p.muted_foreground)
        .child(div().flex_none().child(t("instancesettings.provider.showsAs")))
        .child(
            div()
                .h(px(36.0))
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.0))
                .overflow_hidden()
                .rounded(radius_lg())
                .bg(p.foreground)
                .px(px(12.0))
                .text_sm()
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(p.background)
                .child(icon("building").size(px(16.0)).text_color(p.primary))
                .child(div().truncate().child(t_with(
                    "connect.provider.continueWith",
                    &[("name", Arg::Str(if name.is_empty() { "…" } else { name }))],
                ))),
        )
        .into_any_element()
}

/// Who signed in, as the provider said (the web's `IdentityCard`).
fn identity_card(identity: &pb::SsoIdentity, p: &Palette) -> AnyElement {
    let rows = [
        (t("connect.callback.identity.name"), identity.name.clone()),
        (t("connect.callback.identity.email"), identity.email.clone()),
        (t("connect.callback.identity.subject"), identity.subject.clone()),
    ];
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(12.0))
        .rounded(radius_2xl())
        .border_1()
        .border_color(p.border)
        .bg(alpha(p.muted, 0.4))
        .text_sm()
        .children(rows.into_iter().filter(|(_, v)| !v.is_empty()).enumerate().map(|(n, (label, value))| {
            motion::rise(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(12.0))
                    .child(div().flex_none().text_xs().text_color(p.muted_foreground).child(label))
                    .child(div().min_w_0().truncate().font_weight(FontWeight::BOLD).child(value)),
                SharedString::from(format!("identity-{n}")),
                Duration::from_millis(150 + 70 * n as u64),
                0.0,
            )
        }))
        .into_any_element()
}

/// Lights a field up for a moment after metadata filled it.
fn glow(el: gpui_kit::Div, filled: u32, delay_ms: u64, p: &Palette) -> AnyElement {
    if filled == 0 {
        return el.into_any_element();
    }
    let color = p.primary;
    let delay = delay_ms as f32 / 1000.0;
    motion::once(
        el,
        SharedString::from(format!("sso-glow-{filled}-{delay_ms}")),
        Duration::from_millis(1000 + delay_ms),
        move |el, t| {
            let total = 1.0 + delay;
            let k = ((t * total - delay) / 1.0).clamp(0.0, 1.0);
            let ring = (k * std::f32::consts::PI).sin() * 4.0;
            el.shadow(vec![gpui_kit::BoxShadow {
                color: alpha(color, 0.45),
                offset: gpui_kit::point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(ring),
                inset: false,
            }])
        },
    )
}
