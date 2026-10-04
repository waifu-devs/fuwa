use std::collections::HashSet;

use tonic::{Request, Response, Status};

use super::{Api, Seat, respond, text};
use crate::db::query_one;
use crate::error::{Error, Result};
use crate::id::{new_id, now_ms, timestamp};
use crate::pb::{self, Permission, role_service_server::RoleService};
use crate::permissions::{self, Access, Bits};
use crate::servers::{self as store, Audit, Payload};

/// Most roles one server can have, as Discord has it.
const MAX_ROLES: i64 = 250;

fn color(value: Option<i32>) -> Result<Option<i32>> {
    match value {
        Some(c) if !(0..=0xFF_FFFF).contains(&c) => Err(Error::invalid("a color is 0xRRGGBB")),
        other => Ok(other),
    }
}

/// Permissions as the audit log keeps them: their numbers, comma-separated.
fn audit_bits(bits: Bits) -> String {
    permissions::to_list(bits).iter().map(i32::to_string).collect::<Vec<_>>().join(",")
}

/// A role by id, inside a write, refusing @everyone where it can't be used.
async fn load_role(conn: &turso::Connection, server_id: &str, role_id: &str) -> Result<pb::Role> {
    permissions::role(conn, server_id, &crate::id::parse_id("role_id", role_id)?).await?.ok_or(Error::NotFound("role"))
}

/// Refuses roles ranked at or above the caller's own.
fn check_below(access: &Access, role: &pb::Role) -> Result<()> {
    if access.above(role.position.into()) {
        Ok(())
    } else {
        Err(Error::denied("you can only change roles ranked below your highest role"))
    }
}

/// Moves the roles at and above `from` by `by`, sending each one's change.
async fn shift(
    conn: &turso::Connection,
    server_id: &str,
    from: i32,
    by: i32,
    now: i64,
    events: &mut Vec<Payload>,
) -> Result<()> {
    let moved =
        permissions::roles(conn, server_id).await?.into_iter().filter(|r| r.id != server_id && r.position >= from);
    for mut role in moved {
        role.position += by;
        role.updated_at = Some(timestamp(now));
        conn.execute(
            "UPDATE roles SET position = ?2, updated_at = ?3 WHERE id = ?1",
            (role.id.as_str(), role.position, now),
        )
        .await?;
        events.push(Payload::RoleUpdated(pb::RoleUpdated { role: Some(role) }));
    }
    Ok(())
}

#[tonic::async_trait]
impl RoleService for Api {
    async fn list_roles(
        &self,
        request: Request<pb::ListRolesRequest>,
    ) -> Result<Response<pb::ListRolesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.membership(&account, &request.get_ref().server_id).await?.sdb;
                Ok(pb::ListRolesResponse { roles: permissions::roles(&*sdb.read()?, &sdb.id).await? })
            }
            .await,
        )
    }

    async fn create_role(
        &self,
        request: Request<pb::CreateRoleRequest>,
    ) -> Result<Response<pb::CreateRoleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageRoles).await?;
                let name = text("name", &req.name, 1, 100)?;
                let color = color(req.color)?;
                let bits = permissions::from_list(&req.permissions)?;
                if !access.may_change(bits, access.server) {
                    return Err(Error::denied("you can only give a role permissions you have"));
                }
                let role = sdb
                    .write(&account.id, async |conn, events| {
                        let count = query_one(conn, "SELECT count(*) FROM roles", (), |r| r.get::<i64>(0)).await?;
                        if count.unwrap_or(0) >= MAX_ROLES {
                            return Err(Error::ResourceExhausted(format!("a server can have at most {MAX_ROLES} roles")));
                        }
                        let now = now_ms();
                        shift(conn, &sdb.id, 1, 1, now, events).await?;
                        let role = pb::Role {
                            id: new_id(),
                            server_id: sdb.id.clone(),
                            name: name.clone(),
                            color,
                            position: 1,
                            permissions: permissions::to_list(bits),
                            hoist: req.hoist,
                            mentionable: req.mentionable,
                            created_at: Some(timestamp(now)),
                            updated_at: Some(timestamp(now)),
                        };
                        conn.execute(
                            "INSERT INTO roles (id, name, color, position, permissions, hoist, mentionable, created_at, updated_at)
                             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?7)",
                            (role.id.as_str(), role.name.as_str(), role.color, bits as i64, role.hoist, role.mentionable, now),
                        )
                        .await?;
                        store::audit(conn, &account.id, Audit::new(pb::AuditAction::RoleCreate, &role.id).role(&role.name))
                            .await?;
                        events.push(Payload::RoleCreated(pb::RoleCreated { role: Some(role.clone()) }));
                        Ok(role)
                    })
                    .await?;
                Ok(pb::CreateRoleResponse { role: Some(role) })
            }
            .await,
        )
    }

    async fn update_role(
        &self,
        request: Request<pb::UpdateRoleRequest>,
    ) -> Result<Response<pb::UpdateRoleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageRoles).await?;
                let name = req.name.as_deref().map(|v| text("name", v, 1, 100)).transpose()?;
                let color = color(req.color)?;
                let bits = req.permissions.as_ref().map(|p| permissions::from_list(&p.permissions)).transpose()?;
                let role = sdb
                    .write(&account.id, async |conn, events| {
                        let before = load_role(conn, &sdb.id, &req.role_id).await?;
                        let everyone = before.id == sdb.id;
                        if everyone {
                            if name.is_some()
                                || color.is_some()
                                || req.clear_color
                                || req.hoist.is_some()
                                || req.mentionable.is_some()
                            {
                                return Err(Error::invalid(
                                    "@everyone keeps its name and look; only its permissions change",
                                ));
                            }
                        } else {
                            check_below(&access, &before)?;
                        }
                        let old = permissions::from_list(&before.permissions)?;
                        if let Some(bits) = bits
                            && !access.may_change(old ^ bits, access.server)
                        {
                            return Err(Error::denied("you can only change permissions you have"));
                        }
                        let now = now_ms();
                        conn.execute(
                            "UPDATE roles SET name = coalesce(?2, name),
                             color = CASE WHEN ?3 THEN NULL ELSE coalesce(?4, color) END,
                             permissions = coalesce(?5, permissions), hoist = coalesce(?6, hoist),
                             mentionable = coalesce(?7, mentionable), updated_at = ?8
                             WHERE id = ?1",
                            (
                                before.id.as_str(),
                                name.as_deref(),
                                req.clear_color,
                                color,
                                bits.map(|b| b as i64),
                                req.hoist,
                                req.mentionable,
                                now,
                            ),
                        )
                        .await?;
                        let role = load_role(conn, &sdb.id, &before.id).await?;
                        let shown = |c: Option<i32>| c.map(|c| format!("#{c:06x}")).unwrap_or_default();
                        let entry = Audit::new(pb::AuditAction::RoleUpdate, &role.id)
                            .role(&before.name)
                            .change("name", &before.name, &role.name)
                            .change("color", shown(before.color), shown(role.color))
                            .change("permissions", audit_bits(old), audit_bits(bits.unwrap_or(old)))
                            .change("hoist", before.hoist, role.hoist)
                            .change("mentionable", before.mentionable, role.mentionable);
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        events.push(Payload::RoleUpdated(pb::RoleUpdated { role: Some(role.clone()) }));
                        Ok(role)
                    })
                    .await?;
                Ok(pb::UpdateRoleResponse { role: Some(role) })
            }
            .await,
        )
    }

    async fn delete_role(
        &self,
        request: Request<pb::DeleteRoleRequest>,
    ) -> Result<Response<pb::DeleteRoleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageRoles).await?;
                sdb.write(&account.id, async |conn, events| {
                    let role = load_role(conn, &sdb.id, &req.role_id).await?;
                    if role.id == sdb.id {
                        return Err(Error::invalid("@everyone can't be deleted"));
                    }
                    check_below(&access, &role)?;
                    conn.execute("DELETE FROM roles WHERE id = ?1", [role.id.as_str()]).await?;
                    conn.execute("DELETE FROM member_roles WHERE role_id = ?1", [role.id.as_str()]).await?;
                    conn.execute(
                        "DELETE FROM channel_overwrites WHERE target_id = ?1 AND target = ?2",
                        (role.id.as_str(), pb::OverwriteTarget::Role as i64),
                    )
                    .await?;
                    events.push(Payload::RoleDeleted(pb::RoleDeleted { role_id: role.id.clone() }));
                    shift(conn, &sdb.id, role.position + 1, -1, now_ms(), events).await?;
                    store::audit(conn, &account.id, Audit::new(pb::AuditAction::RoleDelete, &role.id).role(&role.name))
                        .await
                })
                .await?;
                Ok(pb::DeleteRoleResponse {})
            }
            .await,
        )
    }

    async fn reorder_roles(
        &self,
        request: Request<pb::ReorderRolesRequest>,
    ) -> Result<Response<pb::ReorderRolesResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let Seat { sdb, access, .. } = self.with(&account, &req.server_id, Permission::ManageRoles).await?;
                let roles = sdb
                    .write(&account.id, async |conn, events| {
                        let current: Vec<pb::Role> =
                            permissions::roles(conn, &sdb.id).await?.into_iter().filter(|r| r.id != sdb.id).collect();
                        let listed: HashSet<&str> = req.role_ids.iter().map(String::as_str).collect();
                        if listed.len() != req.role_ids.len()
                            || listed.len() != current.len()
                            || !current.iter().all(|r| listed.contains(r.id.as_str()))
                        {
                            return Err(Error::FailedPrecondition(
                                "the roles changed while you were moving them; try again".into(),
                            ));
                        }
                        let now = now_ms();
                        let mut moved = false;
                        let top = req.role_ids.len() as i32;
                        for (i, id) in req.role_ids.iter().enumerate() {
                            let position = top - i as i32;
                            let Some(before) = current.iter().find(|r| &r.id == id) else { continue };
                            if before.position == position {
                                continue;
                            }
                            if !access.above(before.position.into()) || !access.above(position.into()) {
                                return Err(Error::denied(
                                    "only roles ranked below your highest role can move, and only below it",
                                ));
                            }
                            conn.execute(
                                "UPDATE roles SET position = ?2, updated_at = ?3 WHERE id = ?1",
                                (id.as_str(), position, now),
                            )
                            .await?;
                            let mut role = before.clone();
                            role.position = position;
                            role.updated_at = Some(timestamp(now));
                            events.push(Payload::RoleUpdated(pb::RoleUpdated { role: Some(role) }));
                            moved = true;
                        }
                        if moved {
                            store::audit(conn, &account.id, Audit::new(pb::AuditAction::RolesReorder, "")).await?;
                        }
                        permissions::roles(conn, &sdb.id).await
                    })
                    .await?;
                Ok(pb::ReorderRolesResponse { roles })
            }
            .await,
        )
    }

    async fn add_member_role(
        &self,
        request: Request<pb::AddMemberRoleRequest>,
    ) -> Result<Response<pb::AddMemberRoleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let member = self.hand_out(&account, &req.server_id, &req.user_id, &req.role_id, true).await?;
                Ok(pb::AddMemberRoleResponse { member: Some(member) })
            }
            .await,
        )
    }

    async fn remove_member_role(
        &self,
        request: Request<pb::RemoveMemberRoleRequest>,
    ) -> Result<Response<pb::RemoveMemberRoleResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let member = self.hand_out(&account, &req.server_id, &req.user_id, &req.role_id, false).await?;
                Ok(pb::RemoveMemberRoleResponse { member: Some(member) })
            }
            .await,
        )
    }
}

impl Api {
    /// Gives a member a role, or takes it away. Doing what's already done
    /// changes nothing.
    async fn hand_out(
        &self,
        account: &crate::node::Account,
        server_id: &str,
        user_id: &str,
        role_id: &str,
        give: bool,
    ) -> Result<pb::Member> {
        let Seat { sdb, access, .. } = self.with(account, server_id, Permission::ManageRoles).await?;
        sdb.write(&account.id, async |conn, events| {
            let role = load_role(conn, &sdb.id, role_id).await?;
            if role.id == sdb.id {
                return Err(Error::invalid("everyone has @everyone"));
            }
            check_below(&access, &role)?;
            store::member(conn, &sdb.id, user_id).await?.ok_or(Error::NotFound("member"))?;
            let changed = if give {
                conn.execute(
                    "INSERT INTO member_roles (user_id, role_id) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                    (user_id, role.id.as_str()),
                )
                .await?
            } else {
                conn.execute(
                    "DELETE FROM member_roles WHERE user_id = ?1 AND role_id = ?2",
                    (user_id, role.id.as_str()),
                )
                .await?
            };
            let member = store::member(conn, &sdb.id, user_id).await?.ok_or(Error::NotFound("member"))?;
            if changed > 0 {
                let (before, after) = if give { ("", role.id.as_str()) } else { (role.id.as_str(), "") };
                let entry = Audit::new(pb::AuditAction::MemberRolesUpdate, user_id)
                    .role(&role.name)
                    .change("role", before, after);
                store::audit(conn, &account.id, entry).await?;
                events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
            }
            Ok(member)
        })
        .await
        .map(|mut member| {
            store::scrub_sso(&mut member, &account.id, access.has(Permission::ManageServer));
            member
        })
    }
}
