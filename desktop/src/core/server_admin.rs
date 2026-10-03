//! Running a server: its name, picture and words, its invites, who's banned,
//! and the audit log. The calls behind server settings, as in the web app's
//! `actions.ts`.

use std::collections::HashMap;

use tonic::Code;

use crate::core::Core;
use crate::core::api::Problem;
use crate::pb;
use crate::rpc;

fn missing() -> Problem {
    Problem::new(Code::NotFound, "That instance isn't here.")
}

/// What changes about a server; `None` keeps it.
#[derive(Debug, Clone, Default)]
pub struct ServerPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub default_notifications: Option<pb::NotificationLevel>,
    /// A text channel's id, or empty for no join messages.
    pub system_channel_id: Option<String>,
}

/// Someone's account by id, from a list a call sent along.
pub type People = HashMap<String, pb::User>;

fn people(users: Vec<pb::User>) -> People {
    users.into_iter().map(|u| (u.id.clone(), u)).collect()
}

impl Core {
    pub async fn update_server(&self, key: &str, server_id: &str, patch: ServerPatch) -> Result<pb::Server, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.servers(),
            update_server(pb::UpdateServerRequest {
                server_id: server_id.into(),
                name: patch.name,
                description: patch.description,
                icon_url: patch.icon_url,
                default_notifications: patch.default_notifications.map(|l| l as i32),
                system_channel_id: patch.system_channel_id,
                ..Default::default()
            })
        )
        .await?;
        let server = res.server.unwrap_or_default();
        self.shared.instance(key, |i| {
            if let Some(s) = i.servers.iter_mut().find(|s| s.id == server.id) {
                *s = server.clone();
            }
        });
        Ok(server)
    }

    /// The server's invites, and who made them.
    pub async fn invites(&self, key: &str, server_id: &str) -> Result<(Vec<pb::Invite>, People), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.invites(), list_invites(pb::ListInvitesRequest { server_id: server_id.into() })).await?;
        Ok((res.invites, people(res.inviters)))
    }

    pub async fn delete_invite(&self, key: &str, server_id: &str, code: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.invites(), delete_invite(pb::DeleteInviteRequest { server_id: server_id.into(), code: code.into() }))
            .await?;
        Ok(())
    }

    /// Who's banned, newest first, and who banned them.
    pub async fn bans(&self, key: &str, server_id: &str) -> Result<(Vec<pb::Ban>, People), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.servers(), list_bans(pb::ListBansRequest { server_id: server_id.into() })).await?;
        Ok((res.bans, people(res.moderators)))
    }

    pub async fn unban(&self, key: &str, server_id: &str, user_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.servers(),
            unban_member(pb::UnbanMemberRequest { server_id: server_id.into(), user_id: user_id.into() })
        )
        .await?;
        Ok(())
    }

    /// A page of the audit log, newest first: before `before_id` when given,
    /// only by `actor_id` and only of `action` when set.
    pub async fn audit_log(
        &self,
        key: &str,
        server_id: &str,
        before_id: &str,
        actor_id: &str,
        action: pb::AuditAction,
    ) -> Result<(Vec<pb::AuditEntry>, People, bool), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.servers(),
            list_audit_log(pb::ListAuditLogRequest {
                server_id: server_id.into(),
                limit: 50,
                before_id: before_id.into(),
                actor_id: actor_id.into(),
                action: action as i32,
            })
        )
        .await?;
        Ok((res.entries, people(res.users), res.has_more))
    }
}

/// What changes about a channel; `None` keeps it. An empty `parent_id` takes it out of its category.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelPatch {
    pub name: Option<String>,
    pub topic: Option<String>,
    pub parent_id: Option<String>,
    pub slowmode_seconds: Option<i32>,
}

impl Core {
    fn put_channel(&self, key: &str, channel: &pb::Channel) {
        self.shared.instance(key, |i| {
            let list = i.channels.entry(channel.server_id.clone()).or_default();
            list.retain(|c| c.id != channel.id);
            list.push(channel.clone());
            crate::core::store::sort_channels(list);
        });
    }

    pub async fn update_channel(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        patch: ChannelPatch,
    ) -> Result<pb::Channel, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.channels(),
            update_channel(pb::UpdateChannelRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                name: patch.name,
                topic: patch.topic,
                position: None,
                parent_id: patch.parent_id,
                slowmode_seconds: patch.slowmode_seconds,
            })
        )
        .await?;
        let channel = res.channel.unwrap_or_default();
        self.put_channel(key, &channel);
        Ok(channel)
    }

    pub async fn delete_channel(&self, key: &str, server_id: &str, channel_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.channels(),
            delete_channel(pb::DeleteChannelRequest { server_id: server_id.into(), channel_id: channel_id.into() })
        )
        .await?;
        self.shared.instance(key, |i| {
            if let Some(list) = i.channels.get_mut(server_id) {
                list.retain(|c| c.id != channel_id);
                // A category's channels stay, outside any category.
                for c in list.iter_mut().filter(|c| c.parent_id == channel_id) {
                    c.parent_id.clear();
                }
            }
        });
        Ok(())
    }

    /// Replaces who can do what in a channel, every overwrite at once.
    pub async fn set_channel_permissions(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        overwrites: Vec<pb::PermissionOverwrite>,
    ) -> Result<pb::Channel, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.channels(),
            set_channel_permissions(pb::SetChannelPermissionsRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                overwrites,
            })
        )
        .await?;
        let channel = res.channel.unwrap_or_default();
        self.put_channel(key, &channel);
        Ok(channel)
    }
}

/// What changes about a role; `None` keeps it. `color: Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct RolePatch {
    pub name: Option<String>,
    pub color: Option<Option<u32>>,
    pub hoist: Option<bool>,
    pub mentionable: Option<bool>,
    pub permissions: Option<Vec<i32>>,
}

impl Core {
    fn put_role(&self, key: &str, role: &pb::Role) {
        self.shared.instance(key, |i| {
            let list = i.roles.entry(role.server_id.clone()).or_default();
            list.retain(|r| r.id != role.id);
            list.push(role.clone());
            crate::core::store::sort_roles(list);
        });
    }

    fn put_member(&self, key: &str, server_id: &str, member: pb::Member) {
        let Some(user_id) = member.user.as_ref().map(|u| u.id.clone()) else { return };
        self.shared.instance(key, |i| {
            if let Some(list) = i.members.get_mut(server_id)
                && let Some(m) = list.iter_mut().find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id))
            {
                *m = member;
            }
        });
    }

    /// A new role at the bottom of the list, just above @everyone.
    pub async fn create_role(&self, key: &str, server_id: &str, name: &str) -> Result<pb::Role, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.roles(),
            create_role(pb::CreateRoleRequest { server_id: server_id.into(), name: name.into(), ..Default::default() })
        )
        .await?;
        let role = res.role.unwrap_or_default();
        self.put_role(key, &role);
        Ok(role)
    }

    pub async fn update_role(
        &self,
        key: &str,
        server_id: &str,
        role_id: &str,
        patch: RolePatch,
    ) -> Result<pb::Role, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.roles(),
            update_role(pb::UpdateRoleRequest {
                server_id: server_id.into(),
                role_id: role_id.into(),
                name: patch.name,
                clear_color: patch.color == Some(None),
                color: patch.color.flatten().map(|c| c as i32),
                permissions: patch.permissions.map(|permissions| pb::PermissionSet { permissions }),
                hoist: patch.hoist,
                mentionable: patch.mentionable,
            })
        )
        .await?;
        let role = res.role.unwrap_or_default();
        self.put_role(key, &role);
        Ok(role)
    }

    pub async fn delete_role(&self, key: &str, server_id: &str, role_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(api.roles(), delete_role(pb::DeleteRoleRequest { server_id: server_id.into(), role_id: role_id.into() }))
            .await?;
        self.shared.instance(key, |i| {
            if let Some(list) = i.roles.get_mut(server_id) {
                list.retain(|r| r.id != role_id);
            }
            if let Some(list) = i.members.get_mut(server_id) {
                for m in list {
                    m.role_ids.retain(|r| r != role_id);
                }
            }
        });
        Ok(())
    }

    /// Every role but @everyone, highest first.
    pub async fn reorder_roles(&self, key: &str, server_id: &str, role_ids: Vec<String>) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.roles(), reorder_roles(pb::ReorderRolesRequest { server_id: server_id.into(), role_ids })).await?;
        let mut roles = res.roles;
        crate::core::store::sort_roles(&mut roles);
        self.shared.instance(key, |i| {
            i.roles.insert(server_id.to_owned(), roles);
        });
        Ok(())
    }

    /// Gives someone a role, or takes it away.
    pub async fn set_member_role(
        &self,
        key: &str,
        server_id: &str,
        user_id: &str,
        role_id: &str,
        give: bool,
    ) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let member = if give {
            rpc!(
                api.roles(),
                add_member_role(pb::AddMemberRoleRequest {
                    server_id: server_id.into(),
                    user_id: user_id.into(),
                    role_id: role_id.into(),
                })
            )
            .await?
            .member
        } else {
            rpc!(
                api.roles(),
                remove_member_role(pb::RemoveMemberRoleRequest {
                    server_id: server_id.into(),
                    user_id: user_id.into(),
                    role_id: role_id.into(),
                })
            )
            .await?
            .member
        };
        if let Some(member) = member {
            self.put_member(key, server_id, member);
        }
        Ok(())
    }
}

// ───────────────────────── Emoji ─────────────────────────

/// Whether `name` can be an emoji's name: 2 to 32 letters, digits and underscores.
pub fn emoji_name_ok(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A file name made into an emoji name (`Party Cat.png` → `party_cat`), as the web's `nameFromFile`.
pub fn emoji_name_from_file(file: &str) -> String {
    let stem = match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    };
    let mut name = String::new();
    for c in stem.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name: String = name.trim_matches('_').chars().take(32).collect();
    if name.len() >= 2 { name } else { format!("emoji_{}", if name.is_empty() { "1" } else { &name }) }
}

/// `name`, or `name_2`, `name_3`… when one of `taken` (lowercase) already has it.
pub fn unique_emoji_name(name: &str, taken: &std::collections::HashSet<String>) -> String {
    let mut out = name.to_owned();
    let stem: String = name.chars().take(29).collect();
    let mut n = 2;
    while taken.contains(&out.to_lowercase()) {
        out = format!("{stem}_{n}");
        n += 1;
    }
    out
}

impl Core {
    fn put_emojis(&self, key: &str, server_id: &str, change: impl FnOnce(&mut Vec<pb::Emoji>)) {
        self.shared.instance(key, |i| change(i.emojis.entry(server_id.to_owned()).or_default()));
    }

    /// How many emoji the server may hold, or `None` for no cap.
    pub async fn emoji_cap(&self, key: &str, server_id: &str) -> Result<Option<i64>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res =
            rpc!(api.servers(), get_server_usage(pb::GetServerUsageRequest { server_id: server_id.into() })).await?;
        Ok(res.limits.and_then(|l| l.emojis))
    }

    /// Uploads a picture and makes it an emoji.
    pub async fn add_emoji(
        self: &std::sync::Arc<Self>,
        key: &str,
        server_id: &str,
        name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<pb::Emoji, Problem> {
        let url = self.upload_picture(key, pb::MediaPurpose::Emoji, content_type, bytes).await?;
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.emojis(),
            create_emoji(pb::CreateEmojiRequest { server_id: server_id.into(), name: name.into(), url })
        )
        .await?;
        let emoji = res.emoji.unwrap_or_default();
        self.put_emojis(key, server_id, |list| {
            list.retain(|e| e.id != emoji.id);
            list.push(emoji.clone());
        });
        Ok(emoji)
    }

    pub async fn rename_emoji(
        &self,
        key: &str,
        server_id: &str,
        emoji_id: &str,
        name: &str,
    ) -> Result<pb::Emoji, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.emojis(),
            update_emoji(pb::UpdateEmojiRequest {
                server_id: server_id.into(),
                emoji_id: emoji_id.into(),
                name: name.into(),
            })
        )
        .await?;
        let emoji = res.emoji.unwrap_or_default();
        self.put_emojis(key, server_id, |list| {
            if let Some(e) = list.iter_mut().find(|e| e.id == emoji.id) {
                *e = emoji.clone();
            }
        });
        Ok(emoji)
    }

    pub async fn delete_emoji(&self, key: &str, server_id: &str, emoji_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.emojis(),
            delete_emoji(pb::DeleteEmojiRequest { server_id: server_id.into(), emoji_id: emoji_id.into() })
        )
        .await?;
        self.put_emojis(key, server_id, |list| list.retain(|e| e.id != emoji_id));
        Ok(())
    }
}

// ───────────────────────── Webhooks ─────────────────────────

/// What changes about a webhook; `None` keeps it.
#[derive(Debug, Clone, Default)]
pub struct WebhookPatch {
    pub name: Option<String>,
    /// A new upload, or empty for none.
    pub avatar_url: Option<String>,
    pub channel_id: Option<String>,
}

/// The address apps post to, on the instance's own address, as the web's `webhookUrl`.
pub fn webhook_url(instance_url: &str, w: &pb::Webhook) -> String {
    format!("{}/webhooks/{}/{}/{}", instance_url.trim_end_matches('/'), w.server_id, w.id, w.token)
}

impl Core {
    /// The server's webhooks, and who made them.
    pub async fn webhooks(&self, key: &str, server_id: &str) -> Result<(Vec<pb::Webhook>, People), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(api.webhooks(), list_webhooks(pb::ListWebhooksRequest { server_id: server_id.into() })).await?;
        Ok((res.webhooks, people(res.creators)))
    }

    pub async fn create_webhook(
        &self,
        key: &str,
        server_id: &str,
        channel_id: &str,
        name: &str,
    ) -> Result<pb::Webhook, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.webhooks(),
            create_webhook(pb::CreateWebhookRequest {
                server_id: server_id.into(),
                channel_id: channel_id.into(),
                name: name.into(),
                avatar_url: String::new(),
            })
        )
        .await?;
        Ok(res.webhook.unwrap_or_default())
    }

    /// Changes a webhook; the request carries all three fields, so the ones
    /// not changed go as they are.
    pub async fn update_webhook(
        &self,
        key: &str,
        w: &pb::Webhook,
        patch: WebhookPatch,
    ) -> Result<pb::Webhook, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.webhooks(),
            update_webhook(pb::UpdateWebhookRequest {
                server_id: w.server_id.clone(),
                webhook_id: w.id.clone(),
                name: patch.name.unwrap_or_else(|| w.name.clone()),
                avatar_url: patch.avatar_url.unwrap_or_else(|| w.avatar_url.clone()),
                channel_id: patch.channel_id.unwrap_or_else(|| w.channel_id.clone()),
            })
        )
        .await?;
        Ok(res.webhook.unwrap_or_default())
    }

    /// A new secret for its address; the old address stops working.
    pub async fn reset_webhook(&self, key: &str, server_id: &str, webhook_id: &str) -> Result<pb::Webhook, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.webhooks(),
            reset_webhook_token(pb::ResetWebhookTokenRequest {
                server_id: server_id.into(),
                webhook_id: webhook_id.into(),
            })
        )
        .await?;
        Ok(res.webhook.unwrap_or_default())
    }

    pub async fn delete_webhook(&self, key: &str, server_id: &str, webhook_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.webhooks(),
            delete_webhook(pb::DeleteWebhookRequest { server_id: server_id.into(), webhook_id: webhook_id.into() })
        )
        .await?;
        Ok(())
    }

    /// Posts through a webhook the way other apps do, to try it. The post
    /// goes to the instance the webhook lives on, never anywhere else.
    pub async fn test_webhook(&self, key: &str, w: &pb::Webhook, content: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let body = serde_json::to_vec(&serde_json::json!({ "content": content })).unwrap_or_default();
        crate::core::account::send(http::Method::POST, &webhook_url(&api.url, w), "application/json", body).await
    }

    /// The agents you made on this instance, to add with a tap.
    pub async fn my_agents(&self, key: &str) -> Result<Vec<pb::Agent>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.agents(), list_agents(pb::ListAgentsRequest {})).await?.agents)
    }

    /// Adds an agent to a server by its username. It's a member at once.
    pub async fn add_agent(&self, key: &str, server_id: &str, username: &str) -> Result<pb::Member, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.agents(),
            add_agent(pb::AddAgentRequest { server_id: server_id.into(), username: username.into() })
        )
        .await?;
        let member = res.member.unwrap_or_default();
        self.shared.instance(key, |i| {
            let Some(user) = member.user.clone() else { return };
            let list = i.members.entry(server_id.to_owned()).or_default();
            let before = list.len();
            list.retain(|m| !m.user.as_ref().is_some_and(|u| u.id == user.id));
            let added = list.len() == before;
            list.push(member.clone());
            crate::core::store::sort_members(list);
            if added && let Some(server) = i.servers.iter_mut().find(|s| s.id == server_id) {
                server.member_count += 1;
            }
        });
        Ok(member)
    }

    /// The welcome screen as its editor sees it: every channel it names.
    pub async fn set_welcome_screen(
        &self,
        key: &str,
        server_id: &str,
        screen: pb::WelcomeScreen,
    ) -> Result<pb::WelcomeScreen, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.join(),
            set_welcome_screen(pb::SetWelcomeScreenRequest {
                server_id: server_id.into(),
                welcome_screen: Some(screen)
            })
        )
        .await?;
        Ok(res.welcome_screen.unwrap_or_default())
    }

    /// The server's AutoMod rules, oldest first.
    pub async fn automod_rules(&self, key: &str, server_id: &str) -> Result<Vec<pb::AutoModRule>, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        Ok(rpc!(api.automod(), list_auto_mod_rules(pb::ListAutoModRulesRequest { server_id: server_id.into() }))
            .await?
            .rules)
    }

    /// Adds a rule (without an id) or replaces one.
    pub async fn save_automod_rule(
        &self,
        key: &str,
        server_id: &str,
        rule: pb::AutoModRule,
    ) -> Result<pb::AutoModRule, Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.automod(),
            save_auto_mod_rule(pb::SaveAutoModRuleRequest { server_id: server_id.into(), rule: Some(rule) })
        )
        .await?;
        Ok(res.rule.unwrap_or_default())
    }

    pub async fn delete_automod_rule(&self, key: &str, server_id: &str, rule_id: &str) -> Result<(), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        rpc!(
            api.automod(),
            delete_auto_mod_rule(pb::DeleteAutoModRuleRequest { server_id: server_id.into(), rule_id: rule_id.into() })
        )
        .await?;
        Ok(())
    }

    /// What a rule, saved or not, makes of some text: whether it's caught, and by what.
    pub async fn test_automod_rule(
        &self,
        key: &str,
        server_id: &str,
        rule: pb::AutoModRule,
        content: &str,
    ) -> Result<(bool, Vec<String>), Problem> {
        let api = self.api(key).ok_or_else(missing)?;
        let res = rpc!(
            api.automod(),
            test_auto_mod_rule(pb::TestAutoModRuleRequest {
                server_id: server_id.into(),
                rule: Some(rule),
                content: content.into(),
            })
        )
        .await?;
        Ok((res.matched, res.matches))
    }
}

/// Words typed or pasted into a word list: split on commas and new lines,
/// trimmed, lowercased, and only those not there yet, up to `max` in all.
pub fn add_words(list: &[String], typed: &str, max: usize) -> Vec<String> {
    let mut out = list.to_vec();
    for w in typed.split([',', '\n']) {
        let w = w.trim().to_lowercase();
        if !w.is_empty() && !out.contains(&w) && out.len() < max {
            out.push(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_names_from_files() {
        assert_eq!(emoji_name_from_file("Party Cat.png"), "party_cat");
        assert_eq!(emoji_name_from_file("--wow!!--.GIF"), "wow");
        assert_eq!(emoji_name_from_file("x.png"), "emoji_x");
        assert_eq!(emoji_name_from_file("ねこ.webp"), "emoji_1");
        assert!(emoji_name_ok(&emoji_name_from_file(&format!("{}.png", "a".repeat(80)))));
        assert!(emoji_name_ok("ok_2") && !emoji_name_ok("a") && !emoji_name_ok("no way"));

        let w = pb::Webhook { id: "w1".into(), server_id: "s1".into(), token: "t0k".into(), ..Default::default() };
        assert_eq!(webhook_url("https://fuwa.chat/", &w), "https://fuwa.chat/webhooks/s1/w1/t0k");

        let taken = ["blob".to_owned(), "blob_2".to_owned()].into_iter().collect();
        assert_eq!(unique_emoji_name("Blob", &taken), "Blob_3");
        assert_eq!(unique_emoji_name("cat", &taken), "cat");
    }

    #[test]
    fn word_lists_take_typed_and_pasted_words() {
        let list = vec!["cat*".to_owned()];
        assert_eq!(add_words(&list, "  Dog ", 10), ["cat*", "dog"]);
        assert_eq!(add_words(&list, "a, b\nCAT*,,a", 10), ["cat*", "a", "b"]);
        assert_eq!(add_words(&list, "x, y, z", 2), ["cat*", "x"]);
        assert_eq!(add_words(&list, "   ", 10), ["cat*"]);
    }
}
