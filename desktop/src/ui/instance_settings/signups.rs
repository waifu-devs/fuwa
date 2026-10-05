//! Sign-ups: who can get an account here, and what they can make.

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use super::controls::Opt;
use super::controls::count_text;
use crate::core::i18n::t;
use crate::core::instance_admin::{self as admin};
use crate::pb;
use crate::ui::theme::Palette;
use crate::ui::widgets::icon;

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

/// "on" or "off", for a setting's default.
pub(super) fn on_off(on: bool) -> String {
    if on { t("instancesettings.shared.on") } else { t("instancesettings.shared.off") }
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
                        || t("instancesettings.signUps.localOffNeeds"),
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
                    || t("instancesettings.signUps.linkedOffNeeds"),
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
                Input::new(issuer).prefix(icon("flower-2").size(px(15.0)).text_color(p.muted_foreground)),
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
            &count_text(defaults.servers_per_account),
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
        page = page.child(self.setting(
            "mcp",
            &t("instancesettings.nav.mcp"),
            None,
            &["mcp"],
            &on_off(defaults.mcp),
            6,
            self.toggle(
                "mcp",
                draft.mcp,
                false,
                &t("instancesettings.signUps.mcpLabel"),
                &t("instancesettings.signUps.mcpHint"),
                p,
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
            &on_off(defaults.shared_channels),
            7,
            self.toggle(
                "shared-channels",
                draft.shared_channels,
                false,
                &t("instancesettings.signUps.sharedLabel"),
                &t("instancesettings.signUps.sharedHint"),
                p,
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
            &on_off(defaults.profile_effects),
            8,
            self.toggle(
                "profile-effects",
                draft.profile_effects,
                false,
                &t("instancesettings.signUps.effectsLabel"),
                &t("instancesettings.signUps.effectsHint"),
                p,
                cx,
                |d, on| d.profile_effects = on,
            ),
            p,
            cx,
        ));
        if self.instance_has("rich-presence") {
            page = page.child(self.setting(
                "rich-presence",
                &t("instancesettings.nav.richPresence"),
                None,
                &["rich_presence"],
                &on_off(defaults.rich_presence),
                9,
                self.toggle(
                    "rich-presence",
                    draft.rich_presence,
                    false,
                    &t("instancesettings.signUps.presenceLabel"),
                    &t("instancesettings.signUps.presenceHint"),
                    p,
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
