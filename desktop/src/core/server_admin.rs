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
