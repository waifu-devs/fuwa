//! Sign-ups: who can get an account here, and what they can make.

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px};

use super::InstanceSettingsView;
use super::controls::Opt;
use crate::core::i18n::t;
use crate::core::instance_admin::{self as admin};
use crate::pb;
use crate::ui::theme::Palette;
use crate::ui::widgets::icon;

/// The waifu.dev sign-in provider, the default issuer.
pub(super) const WAIFU_DEV_ISSUER: &str = "https://api.waifu.dev";

/// Open, closed or off, for the three kinds of accounts (their values match).
pub(super) fn accounts_label(v: i32) -> &'static str {
    match v {
        1 => "open",
        2 => "closed",
        _ => "off",
    }
}

/// Everyone, admins or nobody, for servers and agents (their values match).
fn creation_label(v: i32) -> &'static str {
    match v {
        1 => "everyone",
        2 => "admins",
        _ => "nobody",
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
                Opt::new(pb::LocalAccounts::Open as i32, "Open", "Anyone can sign up.", "door-open"),
                Opt::new(pb::LocalAccounts::Closed as i32, "Closed", "Existing accounts only.", "door-closed"),
                Opt::new(local_off, "Off", "No standalone accounts.", "lock").unless(
                    !(draft.local_accounts == local_off || admin::linked_works(&draft) || admin::sso_works(&draft)),
                    "Needs waifu.dev sign-in or single sign-on working first.",
                ),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.local_accounts = v),
        );
        let warn = draft.local_accounts == local_off && saved.local_accounts != local_off && me_local;
        page = page.child(self.setting(
            "local-accounts",
            "Standalone accounts",
            Some("A username and password kept on this instance only."),
            &["local_accounts"],
            accounts_label(defaults.local_accounts),
            0,
            div().flex().flex_col().gap(px(12.0)).child(local).when(warn, |el| {
                el.child(self.notice(
                    "local-off",
                    "You sign in here with a password. Once you sign out, you'll need another way in here to get \
                     back in.",
                    p,
                ))
            }),
            p,
            cx,
        ));

        // waifu.dev accounts.
        let linked = self.choice(
            "linked",
            draft.linked_accounts,
            vec![
                Opt::new(pb::LinkedAccounts::Open as i32, "Open", "Anyone with waifu.dev.", "flower-2"),
                Opt::new(pb::LinkedAccounts::Closed as i32, "Closed", "Linked accounts only.", "door-closed"),
                Opt::new(linked_off, "Off", "No waifu.dev sign-in.", "lock").unless(
                    !(draft.linked_accounts == linked_off
                        || draft.local_accounts != local_off
                        || admin::sso_works(&draft)),
                    "Needs standalone accounts on first.",
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
            "waifu.dev accounts",
            Some("People sign in with their waifu.dev account, and get an account here the first time."),
            &["linked_accounts"],
            accounts_label(defaults.linked_accounts),
            1,
            div().flex().flex_col().gap(px(12.0)).child(linked).when(no_https, |el| {
                el.child(self.notice(
                    "linked-https",
                    "waifu.dev can only send people back to an https address. Set the public address under General \
                     to turn this on.",
                    p,
                ))
            }),
            p,
            cx,
        ));

        if let Some(issuer) = self.texts.get("linked_issuer") {
            page = page.child(self.setting(
                "linked-issuer",
                "Sign-in provider",
                Some(
                    "The OpenAuth issuer waifu.dev sign-ins go through. Accounts already linked stay tied to the one \
                     they came from.",
                ),
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
                Opt::new(pb::ServerCreation::Everyone as i32, "Everyone", "Any signed-in account.", "users"),
                Opt::new(pb::ServerCreation::Admins as i32, "Admins", "Instance admins only.", "crown"),
                Opt::new(pb::ServerCreation::Disabled as i32, "Nobody", "No new servers.", "ban"),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.server_creation = v),
        );
        page = page.child(self.setting(
            "server-creation",
            "Who can create servers",
            None,
            &["server_creation"],
            creation_label(defaults.server_creation),
            3,
            servers,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "servers-per-account",
            "Servers per account",
            Some("How many servers one account may own."),
            &["servers_per_account"],
            &admin::count_label(defaults.servers_per_account),
            4,
            self.cap("servers_per_account", "Up to", false, p, window, cx),
            p,
            cx,
        ));

        let agents = self.choice(
            "agent-creation",
            draft.agent_creation,
            vec![
                Opt::new(pb::AgentCreation::Everyone as i32, "Everyone", "Any signed-in person.", "bot"),
                Opt::new(pb::AgentCreation::Admins as i32, "Admins", "Instance admins only.", "crown"),
                Opt::new(pb::AgentCreation::Disabled as i32, "Nobody", "No new agents.", "ban"),
            ],
            p,
            window,
            cx,
            |this, v, _, cx| this.patch(cx, |d| d.agent_creation = v),
        );
        page = page.child(self.setting(
            "agent-creation",
            "Who can make agents",
            Some("Agents are accounts programs drive, such as bots. Agents already made keep working."),
            &["agent_creation"],
            creation_label(defaults.agent_creation),
            5,
            agents,
            p,
            cx,
        ));
        page = page.child(self.setting(
            "mcp",
            "Agents through MCP",
            None,
            &["mcp"],
            if defaults.mcp { "on" } else { "off" },
            6,
            self.toggle(
                "mcp",
                draft.mcp,
                false,
                "Agents can use this instance as an MCP server",
                "AI apps such as Claude reach it at /mcp with an agent's token and get the same permissions the agent \
                 has. Server managers can still pick which agents may use theirs.",
                p,
                cx,
                |d, on| d.mcp = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "shared-channels",
            "Shared channels",
            None,
            &["shared_channels"],
            if defaults.shared_channels { "on" } else { "off" },
            7,
            self.toggle(
                "shared-channels",
                draft.shared_channels,
                false,
                "Servers can share channels with each other",
                "Admins of two servers here can show one channel in both. Turned off, nobody can start a new one; \
                 channels already shared stay until either side ends them.",
                p,
                cx,
                |d, on| d.shared_channels = on,
            ),
            p,
            cx,
        ));
        page = page.child(self.setting(
            "profile-effects",
            "Profile effects",
            None,
            &["profile_effects"],
            if defaults.profile_effects { "on" } else { "off" },
            8,
            self.toggle(
                "profile-effects",
                draft.profile_effects,
                false,
                "People can put an effect on their profile card",
                "Petals, stars and the like, drawn by the app from your theme's colors. Turned off, nobody's shows, \
                 and everyone's pick comes back when it's on again.",
                p,
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
                if defaults.profile_decorations { "on" } else { "off" },
                8,
                self.toggle(
                    "profile-decorations",
                    draft.profile_decorations,
                    false,
                    &t("instancesettings.signUps.decorationsLabel"),
                    &t("instancesettings.signUps.decorationsHint"),
                    p,
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
                "Rich presence",
                None,
                &["rich_presence"],
                if defaults.rich_presence { "on" } else { "off" },
                9,
                self.toggle(
                    "rich-presence",
                    draft.rich_presence,
                    false,
                    "People can show what they're doing",
                    "Games and apps people's desktop apps see, shown to people they share a server with, once each \
                     person turns it on. Kept in memory only. Turned off, nobody's activity shows; statuses still do.",
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
