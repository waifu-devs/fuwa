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
