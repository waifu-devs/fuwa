//! Sign-ups: who can get an account here, and what they can make.

use crate::ui::instance_home::{focus_ring, has_focus};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use super::controls::{Opt, input_box};
use crate::core::i18n::t;
use crate::core::instance_admin::{self as admin};
use crate::pb;
use crate::ui::theme::Palette;

/// The waifu.dev sign-in provider, the default issuer.
pub(super) const WAIFU_DEV_ISSUER: &str = "https://api.waifu.dev";

/// Open, closed or off, for the three kinds of accounts (their values match).
pub(super) fn accounts_label(v: i32) -> String {
    match v {
        1 => t("instancesettings.shared.open"),
        2 => t("instancesettings.shared.closed"),
        _ => t("instancesettings.shared.off"),
    }
}

/// Everyone, admins or nobody, for servers and agents (their values match).
fn creation_label(v: i32) -> String {
    match v {
        1 => t("instancesettings.shared.everyone"),
        2 => t("instancesettings.shared.admins"),
        _ => t("instancesettings.shared.nobody"),
    }
}

/// Where agents' endpoints may be, as the default reads.
fn endpoints_label(v: i32) -> String {
    match pb::AgentEndpoints::try_from(v) {
        Ok(pb::AgentEndpoints::Any) => t("instancesettings.signUps.endpointsAny"),
        Ok(pb::AgentEndpoints::Off) => t("instancesettings.signUps.endpointsOff"),
        _ => t("instancesettings.signUps.endpointsPublic"),
    }
}

impl InstanceSettingsView {
    pub(super) fn signups_page(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (Some(draft), Some(config)) = (self.draft.clone(), self.config.clone()) else {
            return div().into_any_element();
        };
        let saved = self.saved().cloned().unwrap_or_default();
        let defaults = config.defaults.clone().unwrap_or_default();
        let me_local = self.core.shared.read(|s| {
            s.instance(&self.key)
                .and_then(|i| i.me.as_ref().map(|m| m.kind == pb::AccountKind::Local as i32))
                .unwrap_or(false)
        });
        let (local_off, linked_off) = (pb::LocalAccounts::Off as i32, pb::LinkedAccounts::Off as i32);
        let mut page = div().flex().flex_col();

        // Standalone accounts.
        let local = self.choice(
            "local",
            draft.local_accounts,
            vec![
                Opt::new(
                    pb::LocalAccounts::Open as i32,
                    t("instancesettings.signUps.open"),
                    t("instancesettings.signUps.localOpenHint"),
                    "door-open",
                ),
                Opt::new(
                    pb::LocalAccounts::Closed as i32,
                    t("instancesettings.signUps.closed"),
                    t("instancesettings.signUps.localClosedHint"),
                    "door-closed",
                ),
                Opt::new(local_off, t("serversettings.shared.off"), t("instancesettings.signUps.localOffHint"), "lock")
                    .unless(
                        !(draft.local_accounts == local_off || admin::linked_works(&draft) || admin::sso_works(&draft)),
                        t("instancesettings.signUps.localOffNeeds"),
                    ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.local_accounts = v),
        );
        let warn = draft.local_accounts == local_off && saved.local_accounts != local_off && me_local;
        page =
            page.child(self.setting(
                "local-accounts",
                &t("instancesettings.nav.localAccounts"),
                Some(&t("instancesettings.signUps.localHint")),
                &["local_accounts"],
                &accounts_label(defaults.local_accounts),
                0,
                div().flex().flex_col().gap(px(12.0)).child(local).when(warn, |el| {
                    el.child(self.notice("local-off", &t("instancesettings.signUps.localOffNotice"), p))
                }),
                p,
                cx,
            ));

        // waifu.dev accounts.
        let linked = self.choice(
            "linked",
            draft.linked_accounts,
            vec![
                Opt::new(
                    pb::LinkedAccounts::Open as i32,
                    t("instancesettings.signUps.open"),
                    t("instancesettings.signUps.linkedOpenHint"),
                    "flower-2",
                ),
                Opt::new(
                    pb::LinkedAccounts::Closed as i32,
                    t("instancesettings.signUps.closed"),
                    t("instancesettings.signUps.linkedClosedHint"),
                    "door-closed",
                ),
                Opt::new(
                    linked_off,
                    t("serversettings.shared.off"),
                    t("instancesettings.signUps.linkedOffHint"),
                    "lock",
                )
                .unless(
                    !(draft.linked_accounts == linked_off
                        || draft.local_accounts != local_off
                        || admin::sso_works(&draft)),
                    t("instancesettings.signUps.linkedOffNeeds"),
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.linked_accounts = v),
        );
        let no_https = draft.linked_accounts != linked_off && !admin::can_return_to(&draft.public_url);
        page = page.child(self.setting(
            "linked-accounts",
            &t("instancesettings.nav.linkedAccounts"),
            Some(&t("instancesettings.signUps.linkedHint")),
            &["linked_accounts"],
            &accounts_label(defaults.linked_accounts),
            1,
            div().flex().flex_col().gap(px(12.0)).child(linked).when(no_https, |el| {
                el.child(self.notice("linked-https", &t("instancesettings.signUps.linkedNotice"), p))
            }),
            p,
            cx,
        ));

        if let Some(issuer) = self.texts.get("linked_issuer") {
            page = page.child(self.setting(
                "linked-issuer",
                &t("instancesettings.nav.linkedIssuer"),
                Some(&t("instancesettings.signUps.issuerHint")),
                &["linked_issuer"],
                &defaults.linked_issuer,
                2,
                focus_ring(
                    input_box(Input::new(issuer).appearance(false), Some("flower-2"), p),
                    has_focus(issuer, window, cx),
                    p,
                ),
                p,
                cx,
            ));
        }

        let servers = self.choice(
            "server-creation",
            draft.server_creation,
            vec![
                Opt::new(
                    pb::ServerCreation::Everyone as i32,
                    t("instancesettings.signUps.everyone"),
                    t("instancesettings.signUps.serverEveryoneHint"),
                    "users",
                ),
                Opt::new(
                    pb::ServerCreation::Admins as i32,
                    t("instancesettings.signUps.admins"),
                    t("instancesettings.signUps.adminsHint"),
                    "crown",
                ),
                Opt::new(
                    pb::ServerCreation::Disabled as i32,
                    t("instancesettings.signUps.nobody"),
                    t("instancesettings.signUps.serverNobodyHint"),
                    "ban",
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.server_creation = v),
        );
        page = page.child(self.setting(
            "server-creation",
            &t("instancesettings.nav.serverCreation"),
            None,
            &["server_creation"],
            &creation_label(defaults.server_creation),
            3,
            servers,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "servers-per-account",
            &t("instancesettings.nav.serversPerAccount"),
            Some(&t("instancesettings.signUps.serversPerAccountHint")),
            &["servers_per_account"],
            &admin::count_label(defaults.servers_per_account),
            4,
            self.cap("servers_per_account", &t("instancesettings.shared.upTo"), false, p, window, cx),
            p,
            cx,
        ));

        let agents = self.choice(
            "agent-creation",
            draft.agent_creation,
            vec![
                Opt::new(
                    pb::AgentCreation::Everyone as i32,
                    t("instancesettings.signUps.everyone"),
                    t("instancesettings.signUps.agentEveryoneHint"),
                    "bot",
                ),
                Opt::new(
                    pb::AgentCreation::Admins as i32,
                    t("instancesettings.signUps.admins"),
                    t("instancesettings.signUps.adminsHint"),
                    "crown",
                ),
                Opt::new(
                    pb::AgentCreation::Disabled as i32,
                    t("instancesettings.signUps.nobody"),
                    t("instancesettings.signUps.agentNobodyHint"),
                    "ban",
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.agent_creation = v),
        );
        page = page.child(self.setting(
            "agent-creation",
            &t("instancesettings.nav.agentCreation"),
            Some(&t("instancesettings.signUps.agentHint")),
            &["agent_creation"],
            &creation_label(defaults.agent_creation),
            5,
            agents,
            p,
            cx,
        ));
        if self.instance_has("agent-endpoints") {
            let value = admin::endpoints_of(&draft);
            let endpoints = self.choice(
                "agent-endpoints",
                value,
                vec![
                    Opt::new(
                        pb::AgentEndpoints::Public as i32,
                        t("instancesettings.signUps.endpointsPublic"),
                        t("instancesettings.signUps.endpointsPublicHint"),
                        "globe",
                    ),
                    Opt::new(
                        pb::AgentEndpoints::Any as i32,
                        t("instancesettings.signUps.endpointsAny"),
                        t("instancesettings.signUps.endpointsAnyHint"),
                        "network",
                    ),
                    Opt::new(
                        pb::AgentEndpoints::Off as i32,
                        t("instancesettings.signUps.endpointsOff"),
                        t("instancesettings.signUps.endpointsOffHint"),
                        "ban",
                    ),
                ],
                p,
                window,
                cx,
                |this, v, _, cx| this.patch(cx, |d| d.agent_endpoints = v),
            );
            let anywhere = value == pb::AgentEndpoints::Any as i32;
            page = page.child(self.setting(
                "agent-endpoints",
                &t("instancesettings.nav.agentEndpoints"),
                Some(&t("instancesettings.signUps.endpointsHint")),
                &["agent_endpoints"],
                &endpoints_label(admin::endpoints_of(&defaults)),
                5,
                div().flex().flex_col().gap(px(12.0)).child(endpoints).when(anywhere, |el| {
                    el.child(self.notice("endpoints-any", &t("instancesettings.signUps.endpointsAnyNotice"), p))
                }),
                p,
                cx,
            ));
        }
        page = page.child(self.setting(
            "mcp",
            &t("instancesettings.nav.mcp"),
            None,
            &["mcp"],
            &t(if defaults.mcp { "instancesettings.shared.on" } else { "instancesettings.shared.off" }),
            6,
            self.toggle(
                "mcp",
                draft.mcp,
                false,
                &t("instancesettings.signUps.mcpLabel"),
                &t("instancesettings.signUps.mcpHint"),
                p,
                window,
                cx,
                |d, on| d.mcp = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "shared-channels",
            &t("serversettings.nav.shared"),
            None,
            &["shared_channels"],
            &t(if defaults.shared_channels { "instancesettings.shared.on" } else { "instancesettings.shared.off" }),
            7,
            self.toggle(
                "shared-channels",
                draft.shared_channels,
                false,
                &t("instancesettings.signUps.sharedLabel"),
                &t("instancesettings.signUps.sharedHint"),
                p,
                window,
                cx,
                |d, on| d.shared_channels = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "profile-effects",
            &t("instancesettings.nav.profileEffects"),
            None,
            &["profile_effects"],
            &t(if defaults.profile_effects { "instancesettings.shared.on" } else { "instancesettings.shared.off" }),
            8,
            self.toggle(
                "profile-effects",
                draft.profile_effects,
                false,
                &t("instancesettings.signUps.effectsLabel"),
                &t("instancesettings.signUps.effectsHint"),
                p,
                window,
                cx,
                |d, on| d.profile_effects = on,
            ),
            p,
            cx,
        ));
        if self.instance_has("profile-items") {
            page = page.child(self.setting(
                "profile-decorations",
                &t("instancesettings.nav.profileDecorations"),
                None,
                &["profile_decorations"],
                &t(if defaults.profile_decorations {
                    "instancesettings.shared.on"
                } else {
                    "instancesettings.shared.off"
                }),
                8,
                self.toggle(
                    "profile-decorations",
                    draft.profile_decorations,
                    false,
                    &t("instancesettings.signUps.decorationsLabel"),
                    &t("instancesettings.signUps.decorationsHint"),
                    p,
                    window,
                    cx,
                    |d, on| d.profile_decorations = on,
                ),
                p,
                cx,
            ));
        }
        if self.instance_has("rich-presence") {
            page = page.child(self.setting(
                "rich-presence",
                &t("instancesettings.nav.richPresence"),
                None,
                &["rich_presence"],
                &t(if defaults.rich_presence { "instancesettings.shared.on" } else { "instancesettings.shared.off" }),
                9,
                self.toggle(
                    "rich-presence",
                    draft.rich_presence,
                    false,
                    &t("instancesettings.signUps.presenceLabel"),
                    &t("instancesettings.signUps.presenceHint"),
                    p,
                    window,
                    cx,
                    |d, on| d.rich_presence = on,
                ),
                p,
                cx,
            ));
        }
        page.into_any_element()
    }
}
