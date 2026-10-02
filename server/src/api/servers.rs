use tonic::{Request, Response, Status};

use super::channels::load_channel;
use super::media::PictureOwner;
use super::messages::post_join;
use super::{Api, can_manage, respond, text, url, users};
use crate::db::{query_all, query_one};
use crate::error::{Error, Result};
use crate::id::{millis, now_ms, timestamp};
use crate::pb::{self, server_service_server::ServerService};
use crate::servers::{
    self as store, Audit, MEMBER_COLUMNS, NewServer, Payload, USER_COLUMNS, UsageChange, audit_changes,
    effective_limits, member_row,
};

/// The longest time-out, as Discord has it: 28 days.
const MAX_TIME_OUT_SECONDS: i64 = 28 * 24 * 60 * 60;
/// How far back a ban can take someone's messages with them: seven days.
const MAX_DELETE_MESSAGE_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Checks `me` can moderate `target`: an owner or admin, ranked above them.
fn outranks(me: &pb::Member, target: &pb::Member) -> Result<()> {
    if target.user.as_ref().is_some_and(|u| Some(u) == me.user.as_ref()) {
        return Err(Error::invalid("you can't do that to yourself"));
    }
    if !can_manage(me) || me.role <= target.role {
        return Err(Error::denied("you can only moderate people ranked below you"));
    }
    Ok(())
}

/// A reason kept in the audit log.
fn reason(value: &str) -> Result<String> {
    text("reason", value, 0, 512)
}

/// A time as the audit log keeps it: unix milliseconds, or empty for none.
fn audit_time(t: Option<&prost_types::Timestamp>) -> String {
    t.map(|t| millis(t).to_string()).unwrap_or_default()
}

/// Takes a member out of the server, inside a write, for leaving, kicks and bans.
async fn remove_member(
    conn: &turso::Connection,
    user_id: &str,
    reason: pb::LeaveReason,
    events: &mut Vec<Payload>,
) -> Result<bool> {
    if conn.execute("DELETE FROM members WHERE user_id = ?1", [user_id]).await? == 0 {
        return Ok(false);
    }
    conn.execute("UPDATE usage SET members = members - 1, updated_at = ?1 WHERE id = 1", [now_ms()]).await?;
    events.push(Payload::MemberLeft(pb::MemberLeft { user_id: user_id.to_string(), reason: reason as i32 }));
    Ok(true)
}

#[tonic::async_trait]
impl ServerService for Api {
    async fn create_server(
        &self,
        request: Request<pb::CreateServerRequest>,
    ) -> Result<Response<pb::CreateServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                match self.app.settings().server_creation {
                    pb::ServerCreation::Disabled | pb::ServerCreation::Unspecified => {
                        return Err(Error::FailedPrecondition("this instance doesn't allow creating servers".into()));
                    }
                    pb::ServerCreation::Admins if !account.admin => {
                        return Err(Error::denied("only this instance's admins can create servers"));
                    }
                    _ => {}
                }
                if let Some(limit) = self.app.settings().limits.servers_per_account
                    && self.app.owned_count(&account.id).await? >= limit
                {
                    return Err(Error::ResourceExhausted(format!("an account can own at most {limit} servers here")));
                }
                let req = request.into_inner();
                let new = NewServer {
                    name: text("name", &req.name, 1, 100)?,
                    description: text("description", &req.description, 0, 1000)?,
                    icon_url: url("icon_url", &req.icon_url)?,
                    discoverable: req.discoverable,
                };
                let icon = self.check_picture(&account, pb::MediaPurpose::ServerIcon, &new.icon_url).await?;
                let server = self.app.create_server(&account.user(), new).await?;
                self.keep_picture(icon.as_deref(), Some(&server.id)).await;
                tracing::info!(server = %server.id, owner = %account.id, "server created");
                Ok(pb::CreateServerResponse { server: Some(server) })
            }
            .await,
        )
    }

    async fn get_server(
        &self,
        request: Request<pb::GetServerRequest>,
    ) -> Result<Response<pb::GetServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
                let server = sdb.server().await?;
                if !server.discoverable
                    && !account.admin
                    && store::member(&sdb.read()?, &sdb.id, &account.id).await?.is_none()
                {
                    return Err(Error::NotFound("server"));
                }
                Ok(pb::GetServerResponse { server: Some(server) })
            }
            .await,
        )
    }

    async fn list_servers(
        &self,
        request: Request<pb::ListServersRequest>,
    ) -> Result<Response<pb::ListServersResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                Ok(pb::ListServersResponse { servers: self.app.index.joined(&account.id) })
            }
            .await,
        )
    }

    async fn discover_servers(
        &self,
        request: Request<pb::DiscoverServersRequest>,
    ) -> Result<Response<pb::DiscoverServersResponse>, Status> {
        respond(
            async {
                self.account(request.metadata()).await?;
                Ok(pb::DiscoverServersResponse { servers: self.app.index.discoverable() })
            }
            .await,
        )
    }

    async fn update_server(
        &self,
        request: Request<pb::UpdateServerRequest>,
    ) -> Result<Response<pb::UpdateServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
                let name = req.name.as_deref().map(|v| text("name", v, 1, 100)).transpose()?;
                let description = req.description.as_deref().map(|v| text("description", v, 0, 1000)).transpose()?;
                let icon_url = req.icon_url.as_deref().map(|v| url("icon_url", v)).transpose()?;
                let old_icon = sdb.server().await?.icon_url;
                let new_icon = match icon_url.as_deref().filter(|url| *url != old_icon) {
                    Some(url) => self.check_picture(&account, pb::MediaPurpose::ServerIcon, url).await?,
                    None => None,
                };
                self.keep_picture(new_icon.as_deref(), Some(&sdb.id)).await;
                if let Some(level) = req.default_notifications
                    && !matches!(
                        pb::NotificationLevel::try_from(level),
                        Ok(pb::NotificationLevel::Unspecified | pb::NotificationLevel::All | pb::NotificationLevel::Mentions)
                    )
                {
                    return Err(Error::invalid("servers can default to all messages or only @mentions"));
                }
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        let before = store::load_server(conn).await?;
                        if let Some(channel_id) = req.system_channel_id.as_deref().filter(|id| !id.is_empty()) {
                            let channel = load_channel(conn, &sdb.id, channel_id)
                                .await?
                                .ok_or(Error::NotFound("system channel"))?;
                            if !matches!(
                                pb::ChannelType::try_from(channel.r#type),
                                Ok(pb::ChannelType::Text | pb::ChannelType::Announcement)
                            ) {
                                return Err(Error::invalid("system messages can only go in a text channel"));
                            }
                        }
                        conn.execute(
                            "UPDATE server SET name = coalesce(?1, name), description = coalesce(?2, description),
                         icon_url = coalesce(?3, icon_url), discoverable = coalesce(?4, discoverable),
                         default_notifications = coalesce(?5, default_notifications),
                         system_channel_id = CASE WHEN ?6 IS NULL THEN system_channel_id WHEN ?6 = '' THEN NULL ELSE ?6 END,
                         updated_at = ?7",
                            (
                                name,
                                description,
                                icon_url,
                                req.discoverable,
                                req.default_notifications,
                                req.system_channel_id.as_deref(),
                                now_ms(),
                            ),
                        )
                        .await?;
                        let server = store::load_server(conn).await?;
                        let entry = Audit::new(pb::AuditAction::ServerUpdate, "")
                            .change("name", &before.name, &server.name)
                            .change("description", &before.description, &server.description)
                            .change("icon_url", &before.icon_url, &server.icon_url)
                            .change("discoverable", before.discoverable, server.discoverable)
                            .change("default_notifications", before.default_notifications, server.default_notifications)
                            .change("system_channel_id", &before.system_channel_id, &server.system_channel_id);
                        if !entry.changes.is_empty() {
                            store::audit(conn, &account.id, entry).await?;
                        }
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        Ok(server)
                    })
                    .await?;
                self.app.server_changed(&server).await;
                self.drop_picture(&old_icon, &server.icon_url, PictureOwner::Server(&server.id)).await;
                Ok(pb::UpdateServerResponse { server: Some(server) })
            }
            .await,
        )
    }

    async fn delete_server(
        &self,
        request: Request<pb::DeleteServerRequest>,
    ) -> Result<Response<pb::DeleteServerResponse>, Status> {
        respond(
            async {
                let viewer = self.viewer(request.metadata()).await?;
                let server_id = &request.get_ref().server_id;
                let sdb = self.app.servers.get(server_id).await?;
                let actor = match viewer.account() {
                    Ok(account) => account.id.clone(),
                    Err(_) => String::new(),
                };
                let owner = sdb.server().await?.owner_id;
                if !viewer.is_instance_admin() && owner != actor {
                    return Err(Error::denied("only the server's owner can delete it"));
                }
                self.app.servers.delete(&sdb.id, &actor).await?;
                self.app.server_gone(&sdb.id).await;
                tracing::info!(server = %sdb.id, by = %actor, "server deleted");
                Ok(pb::DeleteServerResponse {})
            }
            .await,
        )
    }

    async fn join_server(
        &self,
        request: Request<pb::JoinServerRequest>,
    ) -> Result<Response<pb::JoinServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let sdb = self.app.servers.get(&request.get_ref().server_id).await?;
                let server = sdb.server().await?;
                if !server.discoverable {
                    return Err(Error::NotFound("server"));
                }
                let limits = sdb.limits(&self.app.settings().limits).await?;
                let user = account.user();
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        if store::member(conn, &sdb.id, &user.id).await?.is_some() {
                            return Err(Error::AlreadyExists("you're already a member".into()));
                        }
                        if query_one(conn, "SELECT 1 FROM bans WHERE user_id = ?1", [user.id.as_str()], |r| {
                            r.get::<i64>(0)
                        })
                        .await?
                        .is_some()
                        {
                            return Err(Error::denied("you're banned from this server"));
                        }
                        if let Some(limit) = limits.members
                            && store::usage_count(conn, "members").await? >= limit
                        {
                            return Err(Error::ResourceExhausted(format!("this server is full ({limit} members)")));
                        }
                        let now = now_ms();
                        let member = store::add_member(conn, &user, pb::MemberRole::Member, &sdb.id, now).await?;
                        events.push(Payload::MemberJoined(pb::MemberJoined { member: Some(member.clone()) }));
                        post_join(conn, &store::load_server(conn).await?, &user.id, now, events).await?;
                        Ok(member)
                    })
                    .await?;
                self.app.membership_changed(&account.id, &sdb.id, true).await;
                Ok(pb::JoinServerResponse { server: Some(sdb.server().await?), member: Some(member) })
            }
            .await,
        )
    }

    async fn leave_server(
        &self,
        request: Request<pb::LeaveServerRequest>,
    ) -> Result<Response<pb::LeaveServerResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (sdb, member) = self.membership(&account, &request.get_ref().server_id).await?;
                if member.role == pb::MemberRole::Owner as i32 {
                    return Err(Error::FailedPrecondition("the owner can't leave; delete the server instead".into()));
                }
                sdb.write(&account.id, async |conn, events| {
                    // Leaving twice at once: the second finds nothing to take away.
                    if !remove_member(conn, &account.id, pb::LeaveReason::Left, events).await? {
                        return Err(Error::NotFound("membership"));
                    }
                    Ok(())
                })
                .await?;
                self.app.membership_changed(&account.id, &sdb.id, false).await;
                self.forget_notifications(&sdb.id, None, Some(&account.id)).await;
                Ok(pb::LeaveServerResponse {})
            }
            .await,
        )
    }

    async fn list_members(
        &self,
        request: Request<pb::ListMembersRequest>,
    ) -> Result<Response<pb::ListMembersResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (sdb, _) = self.membership(&account, &request.get_ref().server_id).await?;
                let conn = sdb.read()?;
                let members = query_all(
                    &conn,
                    &format!(
                        "SELECT {MEMBER_COLUMNS} FROM members JOIN users ON users.id = members.user_id
                     ORDER BY members.role DESC, users.display_name"
                    ),
                    (),
                    member_row(&sdb.id),
                )
                .await?;
                Ok(pb::ListMembersResponse { members })
            }
            .await,
        )
    }

    async fn update_member(
        &self,
        request: Request<pb::UpdateMemberRequest>,
    ) -> Result<Response<pb::UpdateMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, me) = self.membership(&account, &req.server_id).await?;
                let target_id = if req.user_id.is_empty() { account.id.clone() } else { req.user_id.clone() };
                let nickname = req.nickname.as_deref().map(|v| text("nickname", v, 0, 32)).transpose()?;
                if let Some(role) = req.role {
                    if me.role != pb::MemberRole::Owner as i32 {
                        return Err(Error::denied("only the server's owner can change roles"));
                    }
                    if target_id == account.id {
                        return Err(Error::invalid("hand the server to someone else to stop being its owner"));
                    }
                    if !matches!(pb::MemberRole::try_from(role), Ok(pb::MemberRole::Member | pb::MemberRole::Admin)) {
                        return Err(Error::invalid("members can be made admins or members; ownership moves on its own"));
                    }
                }
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        let target =
                            store::member(conn, &sdb.id, &target_id).await?.ok_or(Error::NotFound("member"))?;
                        if target_id != account.id && !(can_manage(&me) && me.role > target.role) {
                            return Err(Error::denied("you can only change people ranked below you"));
                        }
                        conn.execute(
                            "UPDATE members SET nickname = coalesce(?2, nickname), role = coalesce(?3, role) WHERE user_id = ?1",
                            (target_id.as_str(), nickname.as_deref(), req.role),
                        )
                        .await?;
                        let member =
                            store::member(conn, &sdb.id, &target_id).await?.ok_or(Error::NotFound("member"))?;
                        if target_id != account.id {
                            let entry = Audit::new(pb::AuditAction::MemberUpdate, &target_id)
                                .change("nickname", &target.nickname, &member.nickname)
                                .change("role", target.role, member.role);
                            if !entry.changes.is_empty() {
                                store::audit(conn, &account.id, entry).await?;
                            }
                        }
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                Ok(pb::UpdateMemberResponse { member: Some(member) })
            }
            .await,
        )
    }

    async fn get_server_usage(
        &self,
        request: Request<pb::GetServerUsageRequest>,
    ) -> Result<Response<pb::GetServerUsageResponse>, Status> {
        respond(
            async {
                let viewer = self.viewer(request.metadata()).await?;
                let server_id = &request.get_ref().server_id;
                let sdb = if viewer.is_instance_admin() {
                    self.app.servers.get(server_id).await?
                } else {
                    let (sdb, member) = self.membership(viewer.account()?, server_id).await?;
                    if !can_manage(&member) {
                        return Err(Error::denied("only the server's owner and admins can see its usage"));
                    }
                    sdb
                };
                let own = sdb.own_limits().await?;
                Ok(pb::GetServerUsageResponse {
                    usage: Some(sdb.usage().await?),
                    limits: Some(effective_limits(own, &self.app.settings().limits)),
                    own_limits: Some(own),
                })
            }
            .await,
        )
    }

    async fn time_out_member(
        &self,
        request: Request<pb::TimeOutMemberRequest>,
    ) -> Result<Response<pb::TimeOutMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, me) = self.manager(&account, &req.server_id).await?;
                if !(0..=MAX_TIME_OUT_SECONDS).contains(&req.seconds) {
                    return Err(Error::invalid("a time-out can last up to 28 days"));
                }
                let reason = reason(&req.reason)?;
                let member = sdb
                    .write(&account.id, async |conn, events| {
                        let target =
                            store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        outranks(&me, &target)?;
                        let until = (req.seconds > 0).then(|| now_ms() + req.seconds * 1000);
                        conn.execute(
                            "UPDATE members SET timed_out_until = ?2 WHERE user_id = ?1",
                            (req.user_id.as_str(), until),
                        )
                        .await?;
                        let member =
                            store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        let entry = Audit::new(pb::AuditAction::MemberTimeOut, &req.user_id).reason(&reason).change(
                            "timed_out_until",
                            audit_time(target.timed_out_until.as_ref()),
                            until.map(|t| t.to_string()).unwrap_or_default(),
                        );
                        store::audit(conn, &account.id, entry).await?;
                        events.push(Payload::MemberUpdated(pb::MemberUpdated { member: Some(member.clone()) }));
                        Ok(member)
                    })
                    .await?;
                Ok(pb::TimeOutMemberResponse { member: Some(member) })
            }
            .await,
        )
    }

    async fn kick_member(
        &self,
        request: Request<pb::KickMemberRequest>,
    ) -> Result<Response<pb::KickMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, me) = self.manager(&account, &req.server_id).await?;
                let reason = reason(&req.reason)?;
                sdb.write(&account.id, async |conn, events| {
                    let target = store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                    outranks(&me, &target)?;
                    remove_member(conn, &req.user_id, pb::LeaveReason::Kicked, events).await?;
                    store::audit(
                        conn,
                        &account.id,
                        Audit::new(pb::AuditAction::MemberKick, &req.user_id).reason(&reason),
                    )
                    .await?;
                    Ok(())
                })
                .await?;
                self.app.membership_changed(&req.user_id, &sdb.id, false).await;
                self.forget_notifications(&sdb.id, None, Some(&req.user_id)).await;
                tracing::info!(server = %sdb.id, user = %req.user_id, by = %account.id, "member kicked");
                Ok(pb::KickMemberResponse {})
            }
            .await,
        )
    }

    async fn ban_member(
        &self,
        request: Request<pb::BanMemberRequest>,
    ) -> Result<Response<pb::BanMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, me) = self.manager(&account, &req.server_id).await?;
                if !(0..=MAX_DELETE_MESSAGE_SECONDS).contains(&req.delete_message_seconds) {
                    return Err(Error::invalid("a ban can take up to seven days of messages with it"));
                }
                let reason = reason(&req.reason)?;
                let ban = async |conn: &turso::Connection, events: &mut Vec<Payload>| {
                    let user = store::user(conn, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                    let target = store::member(conn, &sdb.id, &req.user_id).await?;
                    if let Some(target) = &target {
                        outranks(&me, target)?;
                    } else if req.user_id == account.id {
                        return Err(Error::invalid("you can't do that to yourself"));
                    }
                    let now = now_ms();
                    match conn
                        .execute(
                            "INSERT INTO bans (user_id, reason, banned_by, created_at) VALUES (?1, ?2, ?3, ?4)",
                            (req.user_id.as_str(), reason.as_str(), account.id.as_str(), now),
                        )
                        .await
                        .map_err(Error::from)
                    {
                        Err(err) if crate::db::is_unique_violation(&err) => {
                            return Err(Error::AlreadyExists("they're already banned".into()));
                        }
                        other => other?,
                    };
                    let was_member = remove_member(conn, &req.user_id, pb::LeaveReason::Banned, events).await?;
                    let mut deleted = 0;
                    if req.delete_message_seconds > 0 {
                        let messages = query_all(
                            conn,
                            "SELECT id, channel_id, size, attachment_count FROM messages WHERE author_id = ?1 AND created_at >= ?2",
                            (req.user_id.as_str(), now - req.delete_message_seconds * 1000),
                            |r| Ok((r.get::<String>(0)?, r.get::<String>(1)?, r.get::<i64>(2)?, r.get::<i64>(3)?)),
                        )
                        .await?;
                        let mut change = UsageChange::default();
                        for (id, channel_id, size, attachments) in messages {
                            conn.execute("DELETE FROM messages WHERE id = ?1", [id.as_str()]).await?;
                            change.messages -= 1;
                            change.message_bytes -= size;
                            change.attachments -= attachments;
                            events.push(Payload::MessageDeleted(pb::MessageDeleted { channel_id, message_id: id }));
                            deleted += 1;
                        }
                        if deleted > 0 {
                            store::add_usage(conn, change).await?;
                        }
                    }
                    let entry = Audit::new(pb::AuditAction::MemberBan, &req.user_id)
                        .reason(&reason)
                        .change("deleted_messages", 0, deleted);
                    store::audit(conn, &account.id, entry).await?;
                    let ban = pb::Ban {
                        user: Some(user),
                        reason: reason.clone(),
                        banned_by_id: account.id.clone(),
                        created_at: Some(timestamp(now)),
                    };
                    Ok((ban, deleted, was_member))
                };
                // Taking their messages sweeps rows they could still be adding to.
                let (ban, deleted, was_member) = if req.delete_message_seconds > 0 {
                    sdb.write_alone(&account.id, ban).await?
                } else {
                    sdb.write(&account.id, ban).await?
                };
                if was_member {
                    self.app.membership_changed(&req.user_id, &sdb.id, false).await;
                    self.forget_notifications(&sdb.id, None, Some(&req.user_id)).await;
                }
                tracing::info!(server = %sdb.id, user = %req.user_id, by = %account.id, deleted, "member banned");
                Ok(pb::BanMemberResponse { ban: Some(ban), deleted_messages: deleted })
            }
            .await,
        )
    }

    async fn unban_member(
        &self,
        request: Request<pb::UnbanMemberRequest>,
    ) -> Result<Response<pb::UnbanMemberResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
                sdb.write(&account.id, async |conn, _| {
                    if conn.execute("DELETE FROM bans WHERE user_id = ?1", [req.user_id.as_str()]).await? == 0 {
                        return Err(Error::NotFound("ban"));
                    }
                    store::audit(conn, &account.id, Audit::new(pb::AuditAction::MemberUnban, &req.user_id)).await
                })
                .await?;
                Ok(pb::UnbanMemberResponse {})
            }
            .await,
        )
    }

    async fn list_bans(&self, request: Request<pb::ListBansRequest>) -> Result<Response<pb::ListBansResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let (sdb, _) = self.manager(&account, &request.get_ref().server_id).await?;
                let conn = sdb.read()?;
                let bans = query_all(
                    &conn,
                    &format!(
                        "SELECT {USER_COLUMNS}, bans.reason, bans.banned_by, bans.created_at
                         FROM bans JOIN users ON users.id = bans.user_id ORDER BY bans.created_at DESC"
                    ),
                    (),
                    |r| {
                        Ok(pb::Ban {
                            user: Some(store::user_row(r)?),
                            reason: r.get(7)?,
                            banned_by_id: r.get(8)?,
                            created_at: Some(timestamp(r.get(9)?)),
                        })
                    },
                )
                .await?;
                let ids: Vec<&str> = bans.iter().map(|b| b.banned_by_id.as_str()).collect();
                let moderators = users(&conn, &ids).await?;
                Ok(pb::ListBansResponse { bans, moderators })
            }
            .await,
        )
    }

    async fn list_audit_log(
        &self,
        request: Request<pb::ListAuditLogRequest>,
    ) -> Result<Response<pb::ListAuditLogResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, _) = self.manager(&account, &req.server_id).await?;
                let conn = sdb.read()?;
                let limit = if req.limit <= 0 { 50 } else { req.limit.min(100) } as i64;
                let rows = query_all(
                    &conn,
                    "SELECT id, actor_id, action, target_id, channel_name, reason, changes, created_at FROM audit
                     WHERE (?1 = '' OR id < ?1) AND (?2 = '' OR actor_id = ?2) AND (?3 = 0 OR action = ?3)
                     ORDER BY id DESC LIMIT ?4",
                    (req.before_id.as_str(), req.actor_id.as_str(), req.action as i64, limit + 1),
                    |r| {
                        Ok((
                            pb::AuditEntry {
                                id: r.get(0)?,
                                actor_id: r.get(1)?,
                                action: r.get(2)?,
                                target_id: r.get(3)?,
                                channel_name: r.get(4)?,
                                reason: r.get(5)?,
                                changes: vec![],
                                created_at: Some(timestamp(r.get(7)?)),
                            },
                            r.get::<Option<Vec<u8>>>(6)?,
                        ))
                    },
                )
                .await?;
                let has_more = rows.len() as i64 > limit;
                let entries = rows
                    .into_iter()
                    .take(limit as usize)
                    .map(|(mut entry, changes)| {
                        entry.changes = audit_changes(changes)?;
                        Ok(entry)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let ids: Vec<&str> = entries.iter().flat_map(|e| [e.actor_id.as_str(), e.target_id.as_str()]).collect();
                let users = users(&conn, &ids).await?;
                Ok(pb::ListAuditLogResponse { entries, users, has_more })
            }
            .await,
        )
    }

    async fn transfer_ownership(
        &self,
        request: Request<pb::TransferOwnershipRequest>,
    ) -> Result<Response<pb::TransferOwnershipResponse>, Status> {
        respond(
            async {
                let account = self.account(request.metadata()).await?;
                let req = request.into_inner();
                let (sdb, me) = self.membership(&account, &req.server_id).await?;
                if me.role != pb::MemberRole::Owner as i32 {
                    return Err(Error::denied("only the server's owner can hand it on"));
                }
                if req.user_id == account.id {
                    return Err(Error::invalid("you already own it"));
                }
                if let Some(limit) = self.app.settings().limits.servers_per_account
                    && self.app.owned_count(&req.user_id).await? >= limit
                {
                    return Err(Error::ResourceExhausted(format!(
                        "they already own {limit} servers, the most an account can here"
                    )));
                }
                let server = sdb
                    .write(&account.id, async |conn, events| {
                        store::member(conn, &sdb.id, &req.user_id).await?.ok_or(Error::NotFound("member"))?;
                        let now = now_ms();
                        conn.execute(
                            "UPDATE server SET owner_id = ?1, updated_at = ?2 WHERE owner_id = ?3",
                            (req.user_id.as_str(), now, account.id.as_str()),
                        )
                        .await?;
                        conn.execute(
                            "UPDATE members SET role = ?2 WHERE user_id = ?1",
                            (req.user_id.as_str(), pb::MemberRole::Owner as i64),
                        )
                        .await?;
                        conn.execute(
                            "UPDATE members SET role = ?2 WHERE user_id = ?1",
                            (account.id.as_str(), pb::MemberRole::Admin as i64),
                        )
                        .await?;
                        let entry = Audit::new(pb::AuditAction::OwnershipTransfer, &req.user_id).change(
                            "owner_id",
                            &account.id,
                            &req.user_id,
                        );
                        store::audit(conn, &account.id, entry).await?;
                        let server = store::load_server(conn).await?;
                        events.push(Payload::ServerUpdated(pb::ServerUpdated { server: Some(server.clone()) }));
                        for id in [&req.user_id, &account.id] {
                            let member = store::member(conn, &sdb.id, id).await?;
                            events.push(Payload::MemberUpdated(pb::MemberUpdated { member }));
                        }
                        Ok(server)
                    })
                    .await?;
                self.app.server_changed(&server).await;
                tracing::info!(server = %sdb.id, from = %account.id, to = %req.user_id, "ownership handed on");
                Ok(pb::TransferOwnershipResponse { server: Some(server) })
            }
            .await,
        )
    }
}
